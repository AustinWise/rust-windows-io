use std::io::prelude::*;
use std::net::TcpStream;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum MyError {
    #[error("IO error")]
    IoError(#[from] std::io::Error),
    #[error("short write")]
    ShortWrite,
    #[error("failed to create IOCP: {0}")]
    IocpCreationFailed(std::io::Error),
    #[error("failed to associate handle with IOCP: {0}")]
    IocpAssociationFailed(std::io::Error),
}

const REQUEST: &str = "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: Close\r\n\r\n";

fn send_http_request(sock: &mut TcpStream) -> Result<(), MyError> {
    if sock.write(REQUEST.as_bytes())? != REQUEST.len() {
        return Err(MyError::ShortWrite);
    }
    Ok(())
}

fn read_response(sock: &mut TcpStream) -> Result<String, MyError> {
    let mut buf = String::new();
    sock.read_to_string(&mut buf)?;
    Ok(buf)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut sock = TcpStream::connect("127.0.0.1:8080")?;
    send_http_request(&mut sock)?;
    println!("{}", read_response(&mut sock)?);

    Ok(())
}
