use crate::flatcombining2::DLockDelegate;
use std::{
    cell::SyncUnsafeCell,
    fmt::Debug,
    ptr::read,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering::*},
};

use crossbeam::epoch::{Atomic, Guard, Owned};

#[derive(Debug)]
pub struct Record<T> {
    pub(crate) operation: SyncUnsafeCell<Option<Box<dyn DLockDelegate<T>>>>,
    pub(crate) result: Atomic<T>,
    pub(crate) state: AtomicBool,
    pub(crate) age: AtomicUsize,
    pub(crate) next: Atomic<Record<T>>,
}

unsafe impl<T> Send for Record<T> {}
unsafe impl<T> Sync for Record<T> {}

impl<T: Send> Record<T> {
    fn set_result(&self, value: T) {
        self.result.store(Owned::new(value).with_tag(0), Release);
    }

    #[inline]
    fn get_result(&self, guard: &Guard) -> T {
        unsafe { read(self.result.load(Acquire, guard).deref()) }
    }
}
