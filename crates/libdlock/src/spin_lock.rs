use std::{
    cell::SyncUnsafeCell,
    ops::DerefMut,
    sync::atomic::{AtomicBool, Ordering},
};

use crossbeam::utils::Backoff;
use lock_api::{GuardSend, RawMutex};

use crate::{
    atomic_extension::AtomicExtension,
    dlock::{guard::DLockGuard, DLock, DLockDelegate},
};

#[derive(Debug)]
pub struct RawSpinLock {
    flag: AtomicBool,
}

impl RawSpinLock {
    pub const fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
        }
    }
}

unsafe impl RawMutex for RawSpinLock {
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
    unsafe fn unlock(&self) {
        self.flag.store(false, Ordering::Release);
    }

    const INIT: Self = RawSpinLock::new();

    type GuardMarker = GuardSend;
}

pub type SpinLock<T> = lock_api::Mutex<RawSpinLock, T>;
pub type SpinLockGuard<'a, T> = lock_api::MutexGuard<'a, RawSpinLock, T>;

impl<T> DLock<T> for SpinLock<T> {
    #[cfg(feature = "combiner_stat")]
    fn get_current_thread_combining_time(&self) -> Option<u64> {
        todo!()
    }

    fn lock<'a>(&self, mut f: impl DLockDelegate<T> + 'a) {
        let mut guard = self.lock();
        unsafe {
            f.apply(DLockGuard::new(get_shared(guard.deref_mut())));
        }
    }
}

fn get_shared<T>(ptr: &mut T) -> &SyncUnsafeCell<T> {
    let t = ptr as *mut T as *const SyncUnsafeCell<T>;
    // SAFETY: `T` and `UnsafeCell<T>` have the same memory layout
    unsafe { &*t }
}
