use crate::bindings::{
    windows::win32::debug::GetLastError,
    windows::win32::file_system::SetFileCompletionNotificationModes,
    windows::win32::system_services::{
        CancelThreadpoolIo, CloseThreadpoolIo, CreateThreadpoolIo, StartThreadpoolIo,
        ERROR_IO_PENDING, HANDLE, OVERLAPPED, TP_CALLBACK_INSTANCE, TP_IO,
    },
};

use std::future::Future;
use std::io;
use std::marker::PhantomPinned;
use std::os::windows::io::AsRawSocket;
use std::pin::Pin;
use std::ptr;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

// Represents the result of an IO operation. Maps to the two interesting parameters of
// PTP_WIN32_IO_CALLBACK and GetQueuedCompletionStatus.
#[derive(Clone, Copy)]
pub struct IocpResult {
    io_result: u32,
    number_of_bytes_transferred: usize,
}

impl IocpResult {
    #[allow(dead_code)]
    pub fn new(io_result: u32, number_of_bytes_transferred: usize) -> IocpResult {
        IocpResult {
            io_result,
            number_of_bytes_transferred,
        }
    }

    pub fn new_from_wsa(io_result: i32, number_of_bytes_transferred: u32) -> IocpResult {
        IocpResult {
            io_result: io_result as u32,
            number_of_bytes_transferred: number_of_bytes_transferred as usize,
        }
    }

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

pub extern "system" fn io_completion_function(
    _instance: *mut TP_CALLBACK_INSTANCE,
    _context: *mut ::std::ffi::c_void,
    overlapped: *mut ::std::ffi::c_void,
    io_result: u32,
    number_of_bytes_transferred: usize,
    _io: *mut TP_IO,
) {
    unsafe {
        let mut overlapped = Box::from_raw(overlapped as *mut OverlappedAndIocpStateReference);
        overlapped.process_iocp_completion(io_result, number_of_bytes_transferred);
    }
}

pub struct Tpio {
    tp_io: *mut TP_IO,
}

impl Drop for Tpio {
    fn drop(&mut self) {
        if !self.tp_io.is_null() {
            unsafe {
                CloseThreadpoolIo(self.tp_io);
            }
            self.tp_io = ptr::null_mut();
        }
    }
}

impl Tpio {
    pub fn new(hand: HANDLE) -> io::Result<Tpio> {
        let tp_io = unsafe {
            CreateThreadpoolIo(
                hand,
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
unsafe impl Send for Tpio {}
unsafe impl Sync for Tpio {}

pub fn start_async_io<F>(tp_io: &Tpio, op: F) -> IocpFuture
where
    F: FnOnce(*mut OVERLAPPED) -> IocpResult,
{
    let state = IocpFutureState::new();
    let state = Arc::new(Mutex::new(state));
    unsafe {
        let mut overlapped = Box::pin(OverlappedAndIocpStateReference {
            overlapped: OVERLAPPED {
                internal: 0,
                internal_high: 0,
                anonymous: false,
                h_event: HANDLE::default(),
            },
            state: state.clone(),
            _pin: PhantomPinned,
        });
        StartThreadpoolIo(tp_io.tp_io);
        let mut rc = op(
            &mut Pin::get_unchecked_mut(Pin::as_mut(&mut overlapped)).overlapped as *mut OVERLAPPED,
        );

        if rc.io_result != 0 {
            rc = IocpResult {
                io_result: GetLastError(),
                number_of_bytes_transferred: 0,
            }
        }

        if rc.io_result as i32 == ERROR_IO_PENDING {
            //io_completion_function will take have of cleaning up the Box
            std::mem::forget(overlapped);
        } else {
            //cleanup resources from async IO that never happened
            CancelThreadpoolIo(tp_io.tp_io);

            //propagate results
            let mut mutable_state = state.lock().unwrap();
            mutable_state.result = Some(rc);
        }
    }

    IocpFuture { state }
}

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
        if SetFileCompletionNotificationModes(sock.as_raw_socket().into(), 3).is_err() {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}