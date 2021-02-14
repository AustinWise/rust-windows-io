use bindings::{
    windows::win32::win_sock::{WSARecv, WSASend, WSABUF},
};

use std::convert::TryInto;
use std::io;
use std::net::TcpStream;
use std::net::ToSocketAddrs;
use std::os::windows::io::AsRawSocket;

use crate::iocp_threadpool;
use crate::iocp_threadpool::start_async_io;
use crate::iocp_threadpool::Tpio;

pub struct AsyncTcpStream {
    stream: TcpStream,
    tp_io: Tpio,
}

impl AsyncTcpStream {
    pub(crate) fn new(stream: TcpStream) -> io::Result<AsyncTcpStream> {
        iocp_threadpool::disable_callbacks_on_synchronous_completion(&stream)?;
        let tp_io = iocp_threadpool::Tpio::new(&stream)?;
        Ok(AsyncTcpStream { stream, tp_io })
    }

    pub fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<AsyncTcpStream> {
        Ok(Self::new(TcpStream::connect(addr)?)?)
    }
}

//these are similar to futures::{AsyncRead, AsyncWrite}
impl AsyncTcpStream {
    pub async fn poll_write(&self, buf: &[u8]) -> io::Result<usize> {
        let hand: usize = self.stream.as_raw_socket().try_into().unwrap();

        let ret = start_async_io(&self.tp_io, |overlapped| unsafe {
            let mut wsabuf = WSABUF {
                buf: buf.as_ptr() as *mut i8,
                len: buf.len().try_into().unwrap(),
            };
            let mut sent: u32 = 0;
            let rc = WSASend(hand, &mut wsabuf, 1, &mut sent, 0, overlapped, Option::None);
            if rc == 0 {
                Some(sent as usize)
            } else {
                None
            }
        })
        .await;
        ret.get_number_of_bytes_transferred()
    }

    pub async fn poll_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let hand: usize = self.stream.as_raw_socket().try_into().unwrap();

        let ret = start_async_io(&self.tp_io, |overlapped| unsafe {
            let mut wsabuf = WSABUF {
                buf: buf.as_ptr() as *mut i8,
                len: buf.len().try_into().unwrap(),
            };
            let mut received: u32 = 0;
            let mut flags: u32 = 0;
            let rc = WSARecv(
                hand,
                &mut wsabuf,
                1,
                &mut received,
                &mut flags,
                overlapped,
                Option::None,
            );
            if rc == 0 {
                Some(received as usize)
            } else {
                None
            }
        })
        .await;
        ret.get_number_of_bytes_transferred()
    }

    pub async fn write_all(&self, buf: &[u8]) -> io::Result<()> {
        let mut ndx = 0;
        while ndx < buf.len() {
            let sent = self.poll_write(&buf[ndx..]).await?;
            if sent == 0
            {
                return Err(io::Error::new(io::ErrorKind::Other, "disconnected"));
            }
            ndx += sent;
        }
        Ok(())
    }
}
