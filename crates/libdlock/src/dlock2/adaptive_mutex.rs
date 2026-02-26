//! Adaptive pthread mutex (`PTHREAD_MUTEX_ADAPTIVE_NP`).
//!
//! Linux-specific mutex variant that spins briefly before blocking on
//! contention.  This hybrid approach avoids the overhead of an immediate
//! syscall for short critical sections while still yielding the CPU for
//! long waits.  Common real-world baseline since many production systems
//! use adaptive mutexes.

use std::cell::UnsafeCell;

use super::{DLock2, DLock2Delegate};

/// Adaptive pthread mutex wrapped for the DLock2 interface.
///
/// Uses `PTHREAD_MUTEX_ADAPTIVE_NP` which makes glibc spin briefly before
/// blocking on contention.
pub struct DLock2AdaptiveMutex<T, I, F>
where
    I: Send,
    F: DLock2Delegate<T, I>,
{
    delegate: F,
    data: UnsafeCell<T>,
    mutex: UnsafeCell<libc::pthread_mutex_t>,
    phantom: std::marker::PhantomData<fn() -> I>,
}

impl<T, I, F> std::fmt::Debug for DLock2AdaptiveMutex<T, I, F>
where
    I: Send,
    F: DLock2Delegate<T, I>,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DLock2AdaptiveMutex").finish()
    }
}

impl<T, I, F> std::fmt::Display for DLock2AdaptiveMutex<T, I, F>
where
    I: Send,
    F: DLock2Delegate<T, I>,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AdaptiveMutex")
    }
}

// SAFETY: The mutex protects `data`; only one thread accesses it at a time.
unsafe impl<T: Send, I: Send, F: DLock2Delegate<T, I>> Send for DLock2AdaptiveMutex<T, I, F> {}
unsafe impl<T: Send, I: Send, F: DLock2Delegate<T, I>> Sync for DLock2AdaptiveMutex<T, I, F> {}

impl<T, I, F> DLock2AdaptiveMutex<T, I, F>
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
            let mut attr: libc::pthread_mutexattr_t = std::mem::zeroed();
            libc::pthread_mutexattr_init(&mut attr);
            libc::pthread_mutexattr_settype(&mut attr, libc::PTHREAD_MUTEX_ADAPTIVE_NP);
            libc::pthread_mutex_init(this.mutex.get(), &attr);
            libc::pthread_mutexattr_destroy(&mut attr);
        }
        this
    }
}

impl<T, I, F> Drop for DLock2AdaptiveMutex<T, I, F>
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

unsafe impl<T, I, F> DLock2<I> for DLock2AdaptiveMutex<T, I, F>
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
