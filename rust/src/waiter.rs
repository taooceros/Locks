use std::{
    cell::SyncUnsafeCell,
    hint::spin_loop,
    sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering::*},
    thread::{current, park, Thread},
};

use crossbeam::{atomic::AtomicConsume, utils::Backoff};

pub trait Parker: Default {
    fn wait(&self);
    fn wake(&self);
    fn reset(&self);
    fn prewake(&self);
}

#[derive(Default, Debug)]
pub struct BlockParker {
    flag: AtomicU32,
    handle: SyncUnsafeCell<Option<Thread>>,
}

impl Parker for BlockParker {
    fn wait(&self) {
        unsafe {
            *self.handle.get() = Some(current());
        }

        let backoff = Backoff::new();

        loop {
            let flag = self.flag.load_consume();

            if flag == 2 {
                break
            } else if flag == 1 {
                backoff.snooze();
                continue;
            } else if flag == 0 {
                park()
            }
        }
    }

    fn wake(&self) {
        if self.flag.swap(2, AcqRel) == 0 {
            unsafe {
                match (*self.handle.get()).as_ref() {
                    Some(thread) => thread.unpark(),
                    None => return,
                }
            }
        }
    }

    fn reset(&self) {
        self.flag.store(0, Release);
    }

    fn prewake(&self) {
        if self.flag.fetch_add(1, AcqRel) == 0 {
            unsafe {
                match (*self.handle.get()).as_ref() {
                    Some(thread) => thread.unpark(),
                    None => return,
                }
            }
        }
    }
}

#[derive(Default, Debug)]
pub struct SpinBlockParker {
    flag: AtomicU32,
    handle: SyncUnsafeCell<Option<Thread>>,
}

impl Parker for SpinBlockParker {
    fn wait(&self) {
        unsafe {
            *self.handle.get() = Some(current());
        }

        let mut counter = 100;

        loop {
            let flag = self.flag.load_consume();

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
}

#[derive(Default, Debug)]
pub struct SpinParker {
    flag: AtomicU32,
}

impl Parker for SpinParker {
    fn wait(&self) {
        let backoff = Backoff::default();
        loop {
            let flag = self.flag.load(Relaxed);

            if flag < 1 {
                backoff.snooze()
            } else {
                spin_loop()
            }

            if flag == 2 {
                break;
            }
        }
    }

    fn wake(&self) {
        self.flag.store(2, Relaxed);
    }

    fn reset(&self) {
        self.flag.store(0, Relaxed);
    }

    fn prewake(&self) {
        self.flag.fetch_add(1, Relaxed);
    }
}
