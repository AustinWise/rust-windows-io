# Using Windows Async IO

This repo has some quick and dirty examples of invoking some Windows Async IOs from Rust. Specifically
it shows how to asynchronously send and receive some data over a Rust `TcpStream`, receiving notification
of completion using a IO Completion Port.

## Samples

* [sync](./sync) - Synchronously sends some data using standard Rust APIs
* [reactor](./reactor) - Uses the `GetQueuedCompletionStatus` API. A reactor-style system would use
  this API and manage its own threadpool. This sample does not actually use multiple threads, as
  it this is just a example of the APIs and calling them from Rust
* [io_future_threadpool](./io_future_threadpool) - implements futures for reading and writing
  sockets on top of Windows IO threadpool functions like `StartThreadpoolIo`. There is no executor
  required for this async IO; the futures returned can be awaited using whatever executor you like.
