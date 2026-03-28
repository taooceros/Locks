//! Raw pthread mutex (`PTHREAD_MUTEX_NORMAL`).
//!
//! Unlike Rust's `std::sync::Mutex` which spins ~100 iterations before
//! blocking, glibc's `PTHREAD_MUTEX_NORMAL` goes straight to `futex_wait`
//! on contention.  This makes it a meaningful baseline for measuring the
//! cost of immediate kernel-mediated blocking vs userspace spinning.

use std::cell::UnsafeCell;

use super::{DLock2, DLock2Delegate};

pub struct DLock2PthreadMutex<T, I, F>
where
    I: Send,
    F: DLock2Delegate<T, I>,
{
    delegate: F,
    data: UnsafeCell<T>,
    mutex: UnsafeCell<libc::pthread_mutex_t>,
    phantom: std::marker::PhantomData<fn() -> I>,
}

impl<T, I, F> std::fmt::Debug for DLock2PthreadMutex<T, I, F>
where
    I: Send,
    F: DLock2Delegate<T, I>,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DLock2PthreadMutex").finish()
    }
}

impl<T, I, F> std::fmt::Display for DLock2PthreadMutex<T, I, F>
where
    I: Send,
    F: DLock2Delegate<T, I>,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PthreadMutex")
    }
}

// SAFETY: The mutex protects `data`; only one thread accesses it at a time.
unsafe impl<T: Send, I: Send, F: DLock2Delegate<T, I>> Send for DLock2PthreadMutex<T, I, F> {}
unsafe impl<T: Send, I: Send, F: DLock2Delegate<T, I>> Sync for DLock2PthreadMutex<T, I, F> {}

impl<T, I, F> DLock2PthreadMutex<T, I, F>
where
    I: Send,
    F: DLock2Delegate<T, I>,
{
    pub fn new(data: T, delegate: F) -> Self {
        let this = Self {
            delegate,
            data: UnsafeCell::new(data),
            mutex: UnsafeCell::new(unsafe { std::mem::zeroed() }),
            phantom: std::marker::PhantomData,
        };
        unsafe {
            // PTHREAD_MUTEX_NORMAL is the default, but be explicit.
            let mut attr: libc::pthread_mutexattr_t = std::mem::zeroed();
            libc::pthread_mutexattr_init(&mut attr);
            libc::pthread_mutexattr_settype(&mut attr, libc::PTHREAD_MUTEX_NORMAL);
            libc::pthread_mutex_init(this.mutex.get(), &attr);
            libc::pthread_mutexattr_destroy(&mut attr);
        }
        this
    }
}

impl<T, I, F> Drop for DLock2PthreadMutex<T, I, F>
where
    I: Send,
    F: DLock2Delegate<T, I>,
{
    fn drop(&mut self) {
        unsafe {
            libc::pthread_mutex_destroy(self.mutex.get());
        }
    }
}

unsafe impl<T, I, F> DLock2<I> for DLock2PthreadMutex<T, I, F>
where
    T: Send + Sync,
    I: Send,
    F: DLock2Delegate<T, I>,
{
    fn lock(&self, data: I) -> I {
        unsafe {
            libc::pthread_mutex_lock(self.mutex.get());
            let result = (self.delegate)(&mut *self.data.get(), data);
            libc::pthread_mutex_unlock(self.mutex.get());
            result
        }
    }

    #[cfg(feature = "combiner_stat")]
    fn get_combine_time(&self) -> Option<u64> {
        None
    }
}
