use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;

struct RootInner {
    is_alive: AtomicBool,
}

struct Root {
    inner: Arc<RootInner>,
}

struct Child {
    root: Arc<RootInner>,
}

impl Root {
    fn new() -> Root {
        Root {
            inner: Arc::new(RootInner {
                is_alive: AtomicBool::new(true),
            }),
        }
    }

    fn wait_for_shutdown(self) {
        self.inner.is_alive.store(false, Ordering::SeqCst);
        loop {
            if Arc::strong_count(&self.inner) == 1 {
                break;
            } else {
                thread::sleep_ms(10);
            }
        }
    }
}

impl Child {
    fn new(root: &Root) -> Result<Child, ()> {
        // create the ARC first to in
        let arc = root.inner.clone();
        if root.inner.is_alive.load(Ordering::SeqCst) {
            Ok(Child { root: arc })
        } else {
            Err(())
        }
    }

    fn print(&self) {
        println!(
            "i am child, root alive: {}",
            self.root.is_alive.load(Ordering::Relaxed)
        );
    }
}

impl Drop for RootInner {
    fn drop(&mut self) {
        println!("dropping inner root on {:?}", thread::current().id());
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        self.inner.is_alive.store(false, Ordering::SeqCst);
        println!("dropping root on {:?}", thread::current().id());
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        println!("dropping child on {:?}", thread::current().id());
    }
}

fn main() {
    let root = Root::new();
    let child = Child::new(&root).unwrap();
    let child2 = Child::new(&root).unwrap();
    let t = thread::spawn(move || {
        thread::sleep_ms(500);
        child.print();
    });
    println!("Hello, world!");
    drop(child2);
    root.wait_for_shutdown();
    //drop(root);
    t.join().unwrap();
    // thread::sleep_ms(500);
}
