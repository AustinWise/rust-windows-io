#[allow(dead_code)]
mod bindings {
    ::windows::include_bindings!();
}

use bindings::windows::win32::system_services::HANDLE;

use std::convert::TryInto;
use std::io;

use futures::executor;
use futures::executor::ThreadPool;
use futures::task::SpawnExt;

mod iocp_threadpool;
mod listener;
mod stream;

use stream::AsyncTcpStream;

impl From<std::os::windows::io::RawSocket> for HANDLE {
    fn from(sock: std::os::windows::io::RawSocket) -> Self {
        HANDLE(sock.try_into().unwrap())
    }
}

const REQUEST: &str = "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: Close\r\n\r\n";

async fn do_request() -> io::Result<()> {
    let sock = AsyncTcpStream::connect("127.0.0.1:8080")?;
    sock.poll_write(REQUEST.as_ref()).await?;
    let mut response = [0; 4096];
    let _received = sock.poll_read(&mut response).await?;
    // let _received = _received as usize;
    // println!("{}", String::from_utf8_lossy(&response[0.._received]));
    Ok(())
}

async fn main_inner(pool: &ThreadPool) -> Result<(), Box<dyn std::error::Error>> {
    let mut running_tasks = Vec::new();
    for _i in 0..100 {
        running_tasks.push(pool.spawn_with_handle(do_request())?);
    }

    for subtask in running_tasks {
        subtask.await?;
    }

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = ThreadPool::new().expect("Failed to build pool");
    executor::block_on(main_inner(&pool))?;
    Ok(())
}
