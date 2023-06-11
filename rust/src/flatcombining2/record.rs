use std::{
    fmt::{Debug, Formatter},
    ptr::read,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering::*}, cell::SyncUnsafeCell,
};

use crossbeam::epoch::{Atomic, Guard, Owned};


use crate::{guard::DLockGuard};

#[derive(Debug)]
pub struct Record<T> {
    pub(crate) operation: SyncUnsafeCell<Option<Box<dyn Callable<T>>>>,
    pub(crate) result: Atomic<T>,
    pub(crate) state: AtomicBool,
    pub(crate) age: AtomicUsize,
    pub(crate) next: Atomic<Record<T>>,
}

pub trait Callable<T: Send>: Sync + Send {
    fn call(&mut self, guard: &mut DLockGuard<T>) -> T;
}

impl<T> Debug for dyn Callable<T> {
    fn fmt(&self, _: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        Ok(())
    }
}

impl<T: Send> Record<T> {
    fn set_result(&self, value: T) {
        self.result.store(Owned::new(value).with_tag(0), Release);
    }

    #[inline]
    fn get_result(&self, guard: &Guard) -> T {
        unsafe { read(self.result.load(Acquire, guard).deref()) }
    }
}
