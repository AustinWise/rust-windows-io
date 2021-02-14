use bindings::{
    windows::win32::file_system::{
        CreateIoCompletionPort, GetQueuedCompletionStatus, SetFileCompletionNotificationModes,
    },
    windows::win32::system_services::HANDLE,
    windows::win32::system_services::{ERROR_IO_PENDING, OVERLAPPED},
    windows::win32::win_sock::{WSAGetLastError, WSARecv, WSASend, WSABUF},
    windows::win32::windows_programming::CloseHandle,
};

use std::convert::TryInto;
use std::mem::transmute;
use std::net::TcpStream;
use std::net::ToSocketAddrs;
use std::os::windows::io::AsRawSocket;
use std::ptr;

const INVALID_HANDLE_VALUE: HANDLE = HANDLE(-1);

enum OverlappedResult {
    CompletedSynchronously,
    Pending,
}

fn process_overlapped_return_code(rc: i32) -> Result<OverlappedResult, std::io::Error> {
    if rc == 0 {
        return Ok(OverlappedResult::CompletedSynchronously);
    }
    unsafe {
        let rc = WSAGetLastError();
        if rc == ERROR_IO_PENDING {
            Ok(OverlappedResult::Pending)
        } else {
            Err(std::io::Error::from_raw_os_error(rc))
        }
    }
}

struct WindowsIoCompletionPort {
    handle: HANDLE,
}

impl WindowsIoCompletionPort {
    fn new() -> Result<WindowsIoCompletionPort, std::io::Error> {
        unsafe {
            let handle = CreateIoCompletionPort(INVALID_HANDLE_VALUE, HANDLE::default(), 0, 0);
            if handle == HANDLE::default() {
                return Err(std::io::Error::last_os_error());
            }
            Ok(WindowsIoCompletionPort { handle })
        }
    }

    fn connect_tcp_socket<A: ToSocketAddrs>(
        &self,
        addr: A,
        completion_key: usize,
    ) -> std::io::Result<TcpStream> {
        let sock = TcpStream::connect(addr)?;
        unsafe {
            if CreateIoCompletionPort(sock.as_raw_socket().into(), self.handle, completion_key, 0)
                == HANDLE::default()
            {
                return Err(std::io::Error::last_os_error());
            }
            // 3 = FILE_SKIP_COMPLETION_PORT_ON_SUCCESS | FILE_SKIP_SET_EVENT_ON_HANDLE
            // It prevents a completion from being queued to the IOCP if the operation
            // completes synchronously.
            if !SetFileCompletionNotificationModes(sock.as_raw_socket().into(), 3).as_bool() {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(sock)
    }

    fn wait_for_overlapped_synchronously<F>(
        &self,
        sock: &TcpStream,
        buf: &mut [u8],
        operation: F,
    ) -> Result<u32, std::io::Error>
    where
        F: FnOnce(usize, *mut WSABUF, *mut u32, *mut OVERLAPPED) -> i32,
    {
        // If multiple threads were actually involved, you would need to make sure this overlapped
        // was alive for the duration of the async IO.
        let mut overlapped: Box<OVERLAPPED> = Box::new(Default::default());
        let mut number_of_bytes_transferred: u32 = 0;
        unsafe {
            // the WSASend and WSARecv will make a copy of this buffer,
            // so it's ok to pass a pointer to this location.
            let mut buf = WSABUF {
                buf: transmute(buf.as_ptr()),
                len: buf.len().try_into().unwrap(),
            };
            let rc = operation(
                sock.as_raw_socket().try_into().unwrap(),
                &mut buf,
                &mut number_of_bytes_transferred,
                overlapped.as_mut(),
            );
            match process_overlapped_return_code(rc)? {
                OverlappedResult::CompletedSynchronously => println!("sync!"),
                OverlappedResult::Pending => {
                    // ~~~~~~~~~~~~~~~~~~~~
                    // The entire POINT of IOCP is you don't wait synchronously for IO. But this is a
                    // is an example of how to call IOCP APIs from Rust, so we are doing everything on
                    // one thread.
                    //
                    // A real application would do something like return a promise
                    // from this function and then complete the promise from a threadpool dedicated to
                    // calling `GetQueuedCompletionStatus`. If you did not want to manage a threadpool,
                    // you could use the `StartThreadpoolIo` API which manages the threadpool for you.
                    // ~~~~~~~~~~~~~~~~~~~~
                    println!("async!");
                    let mut completion_key: u32 = 0;
                    let mut returned_overlapped: *mut OVERLAPPED = ptr::null_mut();
                    if !GetQueuedCompletionStatus(
                        self.handle,
                        &mut number_of_bytes_transferred,
                        &mut completion_key,
                        &mut returned_overlapped,
                        1000,
                    )
                    .as_bool()
                    {
                        // TODO: check the returned_overlapped to differentiate between GetQueuedCompletionStatus
                        // failing and the IO operation failing.
                        return Err(std::io::Error::last_os_error());
                    }
                    println!("done!");
                }
            }
        }
        Ok(number_of_bytes_transferred)
    }
}

impl Drop for WindowsIoCompletionPort {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
        self.handle = HANDLE::default();
    }
}

const REQUEST: &str = "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: Close\r\n\r\n";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let iocp = WindowsIoCompletionPort::new()?;
    let mut sock = iocp.connect_tcp_socket("127.0.0.1:8080", 0)?;

    unsafe {
        let mut request_copy = String::from(REQUEST);
        let sent = iocp.wait_for_overlapped_synchronously(
            &mut sock,
            &mut request_copy.as_bytes_mut(),
            |s, wsabuf, bytes_sent, overlapped| {
                WSASend(s, wsabuf, 1, bytes_sent, 0, overlapped, Option::None)
            },
        )?;
        println!("sent: {}", sent);

        let mut response = [0; 4096];
        let received = iocp.wait_for_overlapped_synchronously(
            &mut sock,
            &mut response,
            |s, wsabuf, bytes_sent, overlapped| {
                let mut flags: u32 = 0;
                WSARecv(
                    s,
                    wsabuf,
                    1,
                    bytes_sent,
                    &mut flags,
                    overlapped,
                    Option::None,
                )
            },
        )?;
        println!("received: {}", received);
        let received = received as usize;
        println!("{}", String::from_utf8_lossy(&response[0..received]));
    }

    Ok(())
}
