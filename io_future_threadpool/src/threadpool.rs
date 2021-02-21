use std::ffi::c_void;
use std::future::Future;
use std::io;
use std::marker::PhantomPinned;
use std::panic::catch_unwind;
use std::pin::Pin;
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Arc;
use std::task::{Context, RawWaker, RawWakerVTable, Waker};
use std::mem;

#[allow(unused_imports)]
use bindings::{
    windows::win32::debug::GetLastError,
    windows::win32::file_system::SetFileCompletionNotificationModes,
    windows::win32::system_services::{
        CancelThreadpoolIo, CloseThreadpoolIo, CloseThreadpoolWork, CreateThreadpoolIo,
        CreateThreadpoolWork, StartThreadpoolIo, SubmitThreadpoolWork, ERROR_IO_PENDING,
        OVERLAPPED, PTP_WORK_CALLBACK, TP_CALLBACK_INSTANCE, TP_IO, TP_WORK,
    },
};

struct ThreadpoolWaker {}

unsafe fn clone_waker(raw: *const ()) -> RawWaker {
    unimplemented!();
}

unsafe fn wake_waker(raw: *const ()) {
    unimplemented!();
}

unsafe fn wake_by_ref_waker(raw: *const ()) {
    unimplemented!();
}

unsafe fn drop_waker(raw: *const ()) {
    unimplemented!();
}

const WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(clone_waker, wake_waker, wake_by_ref_waker, drop_waker);

struct WorkItem {
    native: AtomicPtr<TP_WORK>,
    future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
}

impl WorkItem {
    //TODO: find a nicer way of expressing ownership
    #[allow(mutable_transmutes)]
    unsafe fn process(self: Arc<Self>) {
        let waker = Waker::from_raw(RawWaker::new(ptr::null(), &WAKER_VTABLE));
        let mut ctx = Context::from_waker(&waker);
        let mut_self: &mut Self = mem::transmute( self.as_ref());
        mut_self.future.as_mut().poll(&mut ctx);
    }
}

impl Drop for WorkItem {
    fn drop(&mut self) {
        unsafe {
            let tp_work = self.native.load(Ordering::Relaxed);
            if !tp_work.is_null() {
                CloseThreadpoolWork(tp_work);
            }
        }
    }
}

extern "system" fn work_callback(
    instance: *mut TP_CALLBACK_INSTANCE,
    context: *mut ::std::ffi::c_void,
    work: *mut TP_WORK,
) {
    let unwound = catch_unwind(|| unsafe {
        let work = Arc::from_raw(context as *const WorkItem);
        work.process();
    });
    if unwound.is_err() {
        //TODO: is this the right thing to do when a panic happens?
        std::process::abort();
    }
}

pub fn spawn<Fut>(future: Fut) -> io::Result<()>
where
    Fut: Future<Output = ()> + Send + 'static,
{
    let mut work_item = Arc::new(WorkItem {
        native: AtomicPtr::new(ptr::null_mut()),
        future: Box::pin(future),
    });
    let work_item_ptr = Arc::into_raw(work_item.clone());
    let tp_work = unsafe {
        CreateThreadpoolWork(
            Some(work_callback),
            work_item_ptr as *mut c_void,
            ptr::null_mut(),
        )
    };
    if tp_work.is_null() {
        unsafe {
            drop(Arc::from_raw(work_item_ptr));
        }
        return Err(io::Error::last_os_error());
    }
    work_item.native.store(tp_work, Ordering::Relaxed);

    unsafe {
        SubmitThreadpoolWork(tp_work);
    }

    Ok(())
}
