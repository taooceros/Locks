use std::{
    cell::SyncUnsafeCell,
    sync::atomic::{AtomicBool, AtomicU32, Ordering::*},
    thread::{current, park, Thread},
};

use crossbeam::{atomic::AtomicConsume, utils::Backoff};

pub trait Parker: Default {
    fn wait(&self);
    fn wake(&self);
    fn reset(&self);
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

        while self.flag.load_consume() < 2 {
            if self.flag.swap(1, AcqRel) < 2 {
                park()
            }
        }
    }

    fn wake(&self) {
        if self.flag.swap(2, AcqRel) == 1 {
            unsafe {
                (*self.handle.get()).as_ref().unwrap().unpark();
            }
        }
    }

    fn reset(&self) {
        self.flag.store(0, Release);
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

        while counter > 0 {
            if self.flag.load_consume() == 2 {
                return;
            }
            counter -= 1;
        }

        while self.flag.swap(1, Release) < 2 {
            park()
        }
    }

    fn wake(&self) {
        if self.flag.swap(2, AcqRel) == 1 {
            unsafe {
                (*self.handle.get()).as_ref().unwrap().unpark();
            }
        }
    }

    fn reset(&self) {
        self.flag.store(0, Release);
    }
}

#[derive(Default, Debug)]
pub struct SpinParker {
    flag: AtomicBool,
}

impl Parker for SpinParker {
    fn wait(&self) {
        let backoff = Backoff::default();
        while !self.flag.load(Relaxed) {
            backoff.snooze()
        }
    }

    fn wake(&self) {
        self.flag.store(true, Relaxed);
    }

    fn reset(&self) {
        self.flag.store(false, Relaxed);
    }
}
