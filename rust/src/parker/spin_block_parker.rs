use std::sync::atomic::Ordering::*;
use std::thread::park;

use std::hint::spin_loop;

use std::thread::current;
use std::time::Duration;

use super::Parker;

use std::thread::Thread;

use std::cell::SyncUnsafeCell;

use std::sync::atomic::AtomicU32;

#[derive(Default, Debug)]
pub struct SpinBlockParker {
    pub(crate) flag: AtomicU32,
    pub(crate) handle: SyncUnsafeCell<Option<Thread>>,
}

impl Parker for SpinBlockParker {
    fn wait(&self) {
        unsafe {
            *self.handle.get() = Some(current());
        }

        let mut counter = 100;

        loop {
            let flag = self.flag.load(Acquire);

            if flag == 3 {
                break;
            }

            if counter > 0 {
                spin_loop();
                counter -= 1;
                continue;
            }

            if flag < 2 {
                park()
            }
        }

        while self.flag.swap(1, Release) < 2 {
            park()
        }
    }

    fn wake(&self) {
        if self.flag.swap(3, AcqRel) == 1 {
            unsafe {
                (*self.handle.get()).as_ref().unwrap().unpark();
            }
        }
    }

    fn reset(&self) {
        self.flag.store(0, Release);
    }

    fn prewake(&self) {
        let mut old_flag = self.flag.load(Relaxed);

        loop {
            match self
                .flag
                .compare_exchange_weak(old_flag, 2, Relaxed, Relaxed)
            {
                Ok(_) => unsafe {
                    (*self.handle.get()).as_ref().unwrap().unpark();
                    return;
                },
                Err(flag) => {
                    if flag == 3 {
                        return;
                    } else {
                        old_flag = flag;
                    }
                }
            }
        }
    }

    fn wait_timeout(&self, duration: Duration) {
        todo!()
    }
}
