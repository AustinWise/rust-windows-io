use crate::bindings::{
    windows::win32::system_services::HANDLE,
    windows::win32::win_sock::{WSAIoctl, LPFN_ACCEPTEX},
};

use windows::Guid;

use std::convert::TryInto;
use std::ffi::c_void;
use std::io;
use std::net::TcpListener;
use std::net::ToSocketAddrs;
use std::os::windows::io::AsRawSocket;
use std::ptr;

use crate::iocp_threadpool;
use crate::iocp_threadpool::Tpio;
use crate::stream::AsyncTcpStream;

struct AcceptFunctionCache {
    ptr: *mut LPFN_ACCEPTEX,
}

unsafe impl Sync for AcceptFunctionCache {}

impl AcceptFunctionCache {
    fn _get_accept(listener: &TcpListener) -> io::Result<*mut LPFN_ACCEPTEX> {
        const SIO_GET_EXTENSION_FUNCTION_POINTER: u32 = 0xC8000006;
        // WSAID_ACCEPTEX
        let mut guid = Guid::from_values(
            0xb5367df1,
            0xcbac,
            0x11cf,
            [0x95, 0xca, 0x00, 0x80, 0x5f, 0x48, 0xa1, 0x92],
        );
        let mut fnptr: *mut LPFN_ACCEPTEX = ptr::null_mut();
        let mut bytes_returned: u32 = 0;
        let rc: i32;
        unsafe {
            rc = WSAIoctl(
                listener.as_raw_socket() as usize,
                SIO_GET_EXTENSION_FUNCTION_POINTER,
                &mut guid as *mut Guid as *mut c_void,
                std::mem::size_of::<Guid>() as u32,
                &mut fnptr as *mut *mut LPFN_ACCEPTEX as *mut c_void,
                std::mem::size_of::<*mut LPFN_ACCEPTEX>() as u32,
                &mut bytes_returned,
                ptr::null_mut(),
                None,
            );
        }
        if rc == 0 {
            Ok(fnptr)
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn get_accept_for_listener(listener: &TcpListener) -> io::Result<*mut LPFN_ACCEPTEX> {
        unsafe {
            //TODO: there has to be a more Rusty way of making this cache
            static mut ACCEPT_IPV4: AcceptFunctionCache = AcceptFunctionCache {
                ptr: ptr::null_mut(),
            };
            static mut ACCEPT_IPV6: AcceptFunctionCache = AcceptFunctionCache {
                ptr: ptr::null_mut(),
            };
            let local_addr = listener.local_addr()?;
            if local_addr.is_ipv4() {
                if ACCEPT_IPV4.ptr.is_null() {
                    let fntptr = Self::_get_accept(&listener)?;
                    ACCEPT_IPV4.ptr = fntptr;
                    Ok(fntptr)
                } else {
                    Ok(ACCEPT_IPV4.ptr)
                }
            } else if local_addr.is_ipv6() {
                if ACCEPT_IPV6.ptr.is_null() {
                    let fntptr = Self::_get_accept(&listener)?;
                    ACCEPT_IPV6.ptr = fntptr;
                    Ok(fntptr)
                } else {
                    Ok(ACCEPT_IPV6.ptr)
                }
            } else {
                panic!("new version of IP?")
            }
        }
    }
}

#[allow(dead_code)]
pub struct AsyncTcpListener {
    listener: TcpListener,
    tp_io: Tpio,
    accept: *mut LPFN_ACCEPTEX,
}

#[allow(dead_code)]
impl AsyncTcpListener {
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<AsyncTcpListener> {
        let listener = TcpListener::bind(addr)?;
        iocp_threadpool::disable_callbacks_on_synchronous_completion(&listener)?;
        let accept = AcceptFunctionCache::get_accept_for_listener(&listener)?;
        let hand: HANDLE = listener.as_raw_socket().try_into().unwrap();
        let tp_io = iocp_threadpool::Tpio::new(hand)?;
        Ok(AsyncTcpListener {
            listener,
            tp_io,
            accept,
        })
    }

    pub fn accept(&self) -> io::Result<AsyncTcpStream> {
        //TODO: SO_UPDATE_ACCEPT_CONTEXT

        unimplemented!();
    }
}
