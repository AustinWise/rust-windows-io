use std::io;

use futures::executor;
use futures::executor::ThreadPool;
use futures::task::SpawnExt;

mod threadpool;
mod iocp_threadpool;
mod listener;
mod stream;

use listener::AsyncTcpListener;
use stream::AsyncTcpStream;

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

async fn http_client(pool: &ThreadPool) -> Result<(), Box<dyn std::error::Error>> {
    let mut running_tasks = Vec::new();
    for _i in 0..100 {
        running_tasks.push(pool.spawn_with_handle(do_request())?);
    }

    for subtask in running_tasks {
        subtask.await?;
    }

    Ok(())
}

async fn tokio_readme_main(pool: &ThreadPool) -> Result<(), Box<dyn std::error::Error>> {
    let listener = AsyncTcpListener::bind("127.0.0.1:8080")?;

    loop {
        let socket = listener.accept().await?;

        pool.spawn_ok(async move {
            let mut buf = [0; 1024];

            // In a loop, read data from the socket and write the data back.
            loop {
                let n = match socket.poll_read(&mut buf).await {
                    // socket closed
                    Ok(n) if n == 0 => return,
                    Ok(n) => n,
                    Err(e) => {
                        eprintln!("failed to read from socket; err = {:?}", e);
                        return;
                    }
                };

                // Write the data back
                if let Err(e) = socket.write_all(&buf[0..n]).await {
                    eprintln!("failed to write to socket; err = {:?}", e);
                    return;
                }
            }
        });
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = ThreadPool::new().expect("Failed to build pool");
    if std::env::args().any(|a| a == "http") {
        executor::block_on(http_client(&pool))?;
    } else {
        executor::block_on(tokio_readme_main(&pool))?;
    }
    Ok(())
}
