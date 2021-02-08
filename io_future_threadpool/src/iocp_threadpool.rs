use crate::bindings::{
    windows::win32::debug::GetLastError,
    windows::win32::file_system::SetFileCompletionNotificationModes,
    windows::win32::system_services::{
        CancelThreadpoolIo, CloseThreadpoolIo, CreateThreadpoolIo, StartThreadpoolIo,
        ERROR_IO_PENDING, OVERLAPPED, TP_CALLBACK_INSTANCE, TP_IO,
    },
};

use std::convert::TryInto;
use std::future::Future;
use std::io;
use std::marker::PhantomPinned;
use std::os::windows::io::AsRawSocket;
use std::panic::catch_unwind;
use std::pin::Pin;
use std::ptr;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

/// Represents the result of an IO operation. Maps to the two interesting parameters of
/// PTP_WIN32_IO_CALLBACK and GetQueuedCompletionStatus.
#[derive(Clone, Copy)]
pub struct IocpResult {
    io_result: u32,
    number_of_bytes_transferred: usize,
}

impl IocpResult {
    /// Returns an error if the operation failed, otherwise the number of bytes transferred.
    pub fn get_number_of_bytes_transferred(&self) -> io::Result<usize> {
        if self.io_result == 0 {
            Ok(self.number_of_bytes_transferred)
        } else {
            Err(io::Error::from_raw_os_error(self.io_result as i32))
        }
    }
}

pub struct IocpFuture {
    state: Arc<Mutex<IocpFutureState>>,
}

struct IocpFutureState {
    result: Option<IocpResult>,
    waker: Option<Waker>,
}

#[repr(C)]
struct OverlappedAndIocpStateReference {
    overlapped: OVERLAPPED,
    state: Arc<Mutex<IocpFutureState>>,
    //overlapped must not move during the async IO
    _pin: PhantomPinned,
}

impl OverlappedAndIocpStateReference {
    fn process_iocp_completion(&mut self, io_result: u32, number_of_bytes_transferred: usize) {
        let mut mutable_state = self.state.lock().unwrap();
        mutable_state.result = Some(IocpResult {
            io_result,
            number_of_bytes_transferred,
        });
        //TODO: do we have to worry about calling the waker while holding the mutex?
        if let Some(waker) = &mutable_state.waker {
            waker.wake_by_ref();
        };
    }
}

impl IocpFutureState {
    fn new() -> IocpFutureState {
        IocpFutureState {
            result: None,
            waker: None,
        }
    }
}

impl Future for IocpFuture {
    type Output = IocpResult;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut shared_state = self.state.lock().unwrap();
        if let Some(result) = &shared_state.result {
            Poll::Ready(*result)
        } else {
            shared_state.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

extern "system" fn io_completion_function(
    _instance: *mut TP_CALLBACK_INSTANCE,
    _context: *mut ::std::ffi::c_void,
    overlapped: *mut ::std::ffi::c_void,
    io_result: u32,
    number_of_bytes_transferred: usize,
    _io: *mut TP_IO,
) {
    let unwound = catch_unwind(|| unsafe {
        let mut overlapped = Box::from_raw(overlapped as *mut OverlappedAndIocpStateReference);
        overlapped.process_iocp_completion(io_result, number_of_bytes_transferred);
    });
    if unwound.is_err() {
        //TODO: is this the right thing to do when a panic happens?
        std::process::abort();
    }
}

/// Enables receiving asynchronous I/O completion notifications.
pub struct Tpio {
    tp_io: *mut TP_IO,
}

impl Drop for Tpio {
    fn drop(&mut self) {
        // MSDN says:
        //     You should close the associated file handle and wait for all outstanding overlapped
        //     I/O operations to complete before calling this function. You must not cause any more
        //     overlapped I/O operations to occur after calling this function.
        // Maybe we can do something with lifetimes to make sure this is dropped after the socket
        // is closed?
        // TODO: make sure this is dropped after the socket is closed.
        unsafe {
            CloseThreadpoolIo(self.tp_io);
        }
    }
}

impl Tpio {
    /// Creates a new [Tpio] for the given handle. This can be used with [start_async_io] for the
    /// lifetime of the handle.
    pub fn new<T>(sock: &T) -> io::Result<Tpio>
    where
        T: AsRawSocket,
    {
        let tp_io = unsafe {
            CreateThreadpoolIo(
                sock.as_raw_socket().try_into().unwrap(),
                Some(io_completion_function),
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if tp_io.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Tpio { tp_io })
        }
    }
}

// The lifetime of the TP_IO must be at least as long as the handle it is tied to. It's free to move
// between threads during that time. I'm not sure if there is a better way to model that.
// TODO: is this ok?
unsafe impl Send for Tpio {}
unsafe impl Sync for Tpio {}

/// Used to start an async I/O operation. Returns a future the completes when the operation
/// completes.
///
/// # Remarks
///
/// This is a wrapper around the Win32 [`StartThreadpoolIo`](https://docs.microsoft.com/windows/win32/api/threadpoolapiset/nf-threadpoolapiset-startthreadpoolio)
/// API.
///
/// The caller of this function must have first used [disable_callbacks_on_synchronous_completion]
/// on the handle.
///
/// The caller must have previously created one and only one [Tpio] for their handle.
///
/// # Callback
///
/// The callback `op` should call a function that supports overlapped I/O such as `ReadFile`.
/// The provided `OVERLAPPED` should be passed to the Win32 API. No completion routine should be passed.
///
/// If the operation completes synchronously, the call back should return the number of bytes transferred.
/// Otherwise return [None]. `start_async_io` will handle calling `GetLastError` to determine if the
/// I/O is pending or failed.
pub fn start_async_io<F>(tp_io: &Tpio, op: F) -> IocpFuture
where
    F: FnOnce(*mut OVERLAPPED) -> Option<usize>,
{
    let state = Arc::new(Mutex::new(IocpFutureState::new()));
    unsafe {
        let overlapped = Box::new(OverlappedAndIocpStateReference {
            overlapped: Default::default(),
            state: state.clone(),
            _pin: PhantomPinned,
        });
        let overlapped = Box::into_raw(overlapped);
        StartThreadpoolIo(tp_io.tp_io);
        let maybe_sync_completion = op(overlapped as *mut OVERLAPPED);

        let rc = match maybe_sync_completion {
            Some(number_of_bytes_transferred) => IocpResult {
                io_result: 0,
                number_of_bytes_transferred,
            },
            None => IocpResult {
                io_result: GetLastError(),
                number_of_bytes_transferred: 0,
            },
        };

        if rc.io_result as i32 == ERROR_IO_PENDING {
            //io_completion_function will take have of cleaning up the Box
        } else {
            //cleanup resources from async IO that never happened
            CancelThreadpoolIo(tp_io.tp_io);
            drop(Box::from_raw(overlapped));

            //propagate results
            let mut mutable_state = state.lock().unwrap();
            mutable_state.result = Some(rc);
        }
    }

    IocpFuture { state }
}

/// Disables IOCP notifications when a operation completes synchronously. This MUST be called and
/// MUST return Ok before [start_async_io] is called. Failure to do so may result in memory
/// corruption.
pub fn disable_callbacks_on_synchronous_completion<T>(sock: &T) -> io::Result<()>
where
    T: AsRawSocket,
{
    // 3 = FILE_SKIP_COMPLETION_PORT_ON_SUCCESS | FILE_SKIP_SET_EVENT_ON_HANDLE
    // It prevents a completion from being queued to the IOCP if the operation
    // completes synchronously.
    //
    // NOTE: some other runtimes (.NET) handle this call failing and deal with the async notification
    // on synchronous competition. They say:
    //     There is a known bug that exists through Windows 7 with UDP and SetFileCompletionNotificationModes.
    //     So, don't try to enable skipping the completion port on success in this case.
    unsafe {
        if SetFileCompletionNotificationModes(sock.as_raw_socket().into(), 3).as_bool() {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}
