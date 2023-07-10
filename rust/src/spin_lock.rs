use std::{
    cell::{SyncUnsafeCell, UnsafeCell},
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicBool, Ordering},
};

use crossbeam::utils::Backoff;

use crate::{dlock::{DLock, DLockDelegate}, guard::DLockGuard};

use super::RawSimpleLock;

pub struct RawSpinLock {
    flag: AtomicBool,
}

unsafe impl RawSimpleLock for RawSpinLock {
    fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
        }
    }

    #[inline]
    fn try_lock(&self) -> bool {
        self.flag
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    #[inline]
    fn lock(&self) {
        let backoff = Backoff::new();

        while !self.try_lock() {
            backoff.snooze();
        }
    }

    #[inline]
    fn unlock(&self) {
        self.flag.store(false, Ordering::Release);
    }
}

pub struct SpinLock<T> {
    lock: RawSpinLock,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for SpinLock<T> {}
unsafe impl<T: Send> Sync for SpinLock<T> {}

unsafe impl<'s, T> Send for Guard<'s, T> {}
unsafe impl<'s, T: Send + Sync> Sync for Guard<'s, T> {}

pub struct Guard<'s, T> {
    lock: &'s SpinLock<T>,
}

impl<T> SpinLock<T> {
    pub fn new(data: T) -> Self {
        Self {
            lock: RawSpinLock::new(),
            data: UnsafeCell::new(data),
        }
    }

    pub fn lock(&self) -> Guard<T> {
        self.lock.lock();

        Guard { lock: self }
    }
}

impl<'s, T> Deref for Guard<'s, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'s, T> DerefMut for Guard<'s, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'s, T> Drop for Guard<'s, T> {
    fn drop(&mut self) {
        self.lock.lock.unlock();
    }
}

impl<T: Sized> DLock<T> for SpinLock<T> {
    fn lock<'a>(&self, mut f: impl DLockDelegate<T> + 'a) {
        let mut guard = self.lock();
        let data = &mut (*guard) as *mut T as *const SyncUnsafeCell<T>;
        unsafe {
            f.apply(DLockGuard::new(&*data));
        }
    }

    #[cfg(feature = "combiner_stat")]
    fn get_current_thread_combining_time(&self) -> Option<std::num::NonZeroI64> {
        return None;
    }
}
