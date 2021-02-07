use crate::bindings::{
    windows::win32::system_services::HANDLE,
    windows::win32::win_sock::{
        setsockopt, WSAIoctl, WSASocketW, LPFN_ACCEPTEX, LPFN_GETACCEPTEXSOCKADDRS,
    },
};

use windows::Guid;

use std::convert::TryInto;
use std::ffi::c_void;
use std::io;
use std::mem;
use std::net::{SocketAddr, ToSocketAddrs};
use std::net::{TcpListener, TcpStream};
use std::os::windows::io::{AsRawSocket, FromRawSocket, RawSocket};
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

use crate::iocp_threadpool;
use crate::stream::AsyncTcpStream;

struct WsaFunctionCache {
    guid: Guid,
    //We don't need any ordering guarantees when loading or storing to these.
    //It's ok if we do the IOCTL multiple times; it should gives us the same pointer each time.
    ipv4_ptr: AtomicPtr<c_void>,
    ipv6_ptr: AtomicPtr<c_void>,
}

unsafe impl Sync for WsaFunctionCache {}

impl WsaFunctionCache {
    fn get_ptr(&self, listener: &TcpListener) -> io::Result<*mut c_void> {
        let atomic_ptr = match listener.local_addr()? {
            SocketAddr::V4(..) => &self.ipv4_ptr,
            SocketAddr::V6(..) => &self.ipv6_ptr,
        };
        {
            let ret = atomic_ptr.load(Ordering::Relaxed);
            if ret.is_null() {
                return Ok(ret);
            }
        }

        const SIO_GET_EXTENSION_FUNCTION_POINTER: u32 = 0xC8000006;
        let mut guid = self.guid.clone();
        let mut fnptr: *mut c_void = ptr::null_mut();
        let mut bytes_returned: u32 = 0;
        let rc: i32;
        unsafe {
            rc = WSAIoctl(
                listener.as_raw_socket() as usize,
                SIO_GET_EXTENSION_FUNCTION_POINTER,
                &mut guid as *mut Guid as *mut c_void,
                std::mem::size_of::<Guid>() as u32,
                &mut fnptr as *mut *mut c_void as *mut c_void,
                std::mem::size_of::<*mut c_void>() as u32,
                &mut bytes_returned,
                ptr::null_mut(),
                None,
            );
        }
        if rc == 0 {
            atomic_ptr.store(fnptr, Ordering::Relaxed);
            Ok(fnptr)
        } else {
            Err(io::Error::last_os_error())
        }
    }
    fn get_acceptex(listener: &TcpListener) -> io::Result<LPFN_ACCEPTEX> {
        static CACHE: WsaFunctionCache = WsaFunctionCache {
            // WSAID_ACCEPTEX
            guid: Guid::from_values(
                0xb5367df1,
                0xcbac,
                0x11cf,
                [0x95, 0xca, 0x00, 0x80, 0x5f, 0x48, 0xa1, 0x92],
            ),
            ipv4_ptr: AtomicPtr::new(ptr::null_mut()),
            ipv6_ptr: AtomicPtr::new(ptr::null_mut()),
        };
        unsafe { Ok(mem::transmute(CACHE.get_ptr(listener)?)) }
    }

    #[allow(unused)]
    fn get_get_acceptex_sockaddrs(listener: &TcpListener) -> io::Result<LPFN_GETACCEPTEXSOCKADDRS> {
        static CACHE: WsaFunctionCache = WsaFunctionCache {
            // WSAID_GETACCEPTEXSOCKADDRS
            guid: Guid::from_values(
                0xb5367df2,
                0xcbac,
                0x11cf,
                [0x95, 0xca, 0x00, 0x80, 0x5f, 0x48, 0xa1, 0x92],
            ),
            ipv4_ptr: AtomicPtr::new(ptr::null_mut()),
            ipv6_ptr: AtomicPtr::new(ptr::null_mut()),
        };
        unsafe { Ok(mem::transmute(CACHE.get_ptr(listener)?)) }
    }
}

pub struct AsyncTcpListener {
    listener: TcpListener,
    tp_io: iocp_threadpool::Tpio,
    accept_fnptr: LPFN_ACCEPTEX,
}

impl AsyncTcpListener {
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<AsyncTcpListener> {
        let listener = TcpListener::bind(addr)?;
        iocp_threadpool::disable_callbacks_on_synchronous_completion(&listener)?;
        let accept_fnptr = WsaFunctionCache::get_acceptex(&listener)?;
        let hand: HANDLE = listener.as_raw_socket().try_into().unwrap();
        let tp_io = iocp_threadpool::Tpio::new(hand)?;
        Ok(AsyncTcpListener {
            listener,
            tp_io,
            accept_fnptr,
        })
    }

    //TODO: this is roughly based on the Socket code from std. Use that directly somehow?
    fn _create_accept_socket(&self) -> io::Result<RawSocket> {
        const AF_INET: i32 = 2;
        const AF_INET6: i32 = 23;
        const SOCK_STREAM: i32 = 1;
        const IPPROTO_TCP: i32 = 6;
        const WSA_FLAG_OVERLAPPED: u32 = 1;
        const WSA_FLAG_NO_HANDLE_INHERIT: u32 = 0x80;

        let fam = match self.listener.local_addr()? {
            SocketAddr::V4(..) => AF_INET,
            SocketAddr::V6(..) => AF_INET6,
        };

        unsafe {
            let sock = WSASocketW(
                fam,
                SOCK_STREAM,
                IPPROTO_TCP,
                ptr::null_mut(),
                0,
                WSA_FLAG_OVERLAPPED | WSA_FLAG_NO_HANDLE_INHERIT,
            );
            if sock == !0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(sock as RawSocket)
            }
        }
    }

    pub async fn accept(&self) -> io::Result<AsyncTcpStream> {
        let stream: TcpStream;
        unsafe {
            stream = FromRawSocket::from_raw_socket(self._create_accept_socket()?);
        }
        iocp_threadpool::disable_callbacks_on_synchronous_completion(&stream)?;

        let socket_addr_size = 16
            + match self.listener.local_addr()? {
                SocketAddr::V4(..) => 16,
                SocketAddr::V6(..) => 28,
            };

        // Hypothetically if we made this bigger we could receive the incoming connection's initial
        // data. Right now it is only the size of the socket addresses.
        let mut receive_buff: Vec<u8> = vec![0; 2 * socket_addr_size];
        let listener_handle: usize = self.listener.as_raw_socket().try_into().unwrap();
        let accept_handle: usize = stream.as_raw_socket().try_into().unwrap();

        let ret = iocp_threadpool::start_async_io(&self.tp_io, |overlapped| {
            let mut bytes_transferred: u32 = 0;
            let fnptr = self.accept_fnptr;
            let rc = fnptr(
                listener_handle,
                accept_handle,
                receive_buff.as_mut_ptr() as *mut c_void,
                0,
                socket_addr_size as u32,
                socket_addr_size as u32,
                &mut bytes_transferred,
                overlapped,
            );

            if rc.as_bool() {
                Some(bytes_transferred as usize)
            } else {
                None
            }
        })
        .await;

        if 0 != ret.get_number_of_bytes_transferred()? {
            // We did not specify that we wanted data, nor did we make the buffer big enough for any
            // extra data.
            panic!("Received socket data!?");
        }

        //TODO: GetAcceptExSockaddrs to cache it local and remote addresses?

        unsafe {
            const SO_UPDATE_ACCEPT_CONTEXT: i32 = 0x700B;
            const SOL_SOCKET: i32 = 0xffff;
            let ret = setsockopt(
                accept_handle,
                SOL_SOCKET,
                SO_UPDATE_ACCEPT_CONTEXT,
                &listener_handle as *const usize as *const i8,
                std::mem::size_of::<usize>() as i32,
            );
            if ret != 0 {
                return Err(io::Error::from_raw_os_error(ret));
            }
        }

        Ok(AsyncTcpStream::new(stream)?)
    }
}
