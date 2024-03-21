use std::{
    cell::{SyncUnsafeCell, UnsafeCell},
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, MutexGuard,
    },
};

use crossbeam::{atomic::AtomicConsume, utils::Backoff};

use crate::{
    atomic_extension::AtomicExtension,
    dlock::{guard::DLockGuard, DLock, DLockDelegate},
};

use super::RawSimpleLock;

#[derive(Debug)]
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
        while !self.try_lock() {
            let backoff = Backoff::new();

            while self.flag.load_acquire() {
                backoff.spin();
            }
        }
    }

    #[inline]
    fn unlock(&self) {
        self.flag.store(false, Ordering::Release);
    }
}

#[derive(Debug)]
pub struct SpinLock<T> {
    lock: RawSpinLock,
    data: SyncUnsafeCell<T>,
}

unsafe impl<'s, T> Send for Guard<'s, T> {}
unsafe impl<'s, T: Send + Sync> Sync for Guard<'s, T> {}

pub struct Guard<'s, T> {
    lock: &'s SpinLock<T>,
}

impl<T> SpinLock<T> {
    pub fn new(data: T) -> Self {
        Self {
            lock: RawSpinLock::new(),
            data: SyncUnsafeCell::new(data),
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
    fn get_current_thread_combining_time(&self) -> Option<u64> {
        return None;
    }
}
