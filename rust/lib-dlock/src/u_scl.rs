use std::{cell::SyncUnsafeCell, mem::MaybeUninit};

use crate::{
    dlock::guard::DLockGuard,
    dlock::{DLock, DLockDelegate},
    fairlock_acquire, fairlock_init, fairlock_release, fairlock_t, fairlock_thread_init,
};

use self::scl_gurad::SCL_Guard;

#[derive(Debug)]
pub struct USCL<T: ?Sized> {
    lock: SyncUnsafeCell<fairlock_t>,
    data: SyncUnsafeCell<T>,
}

pub mod scl_gurad;

unsafe impl<T: ?Sized + Send> Send for USCL<T> {}
unsafe impl<T: ?Sized + Send> Sync for USCL<T> {}

impl<T> USCL<T> {
    pub fn new(data: T) -> USCL<T> {
        let mut lock = MaybeUninit::<fairlock_t>::uninit();
        unsafe {
            fairlock_init(lock.as_mut_ptr());
        }

        USCL {
            lock: SyncUnsafeCell::new(unsafe { lock.assume_init() }),
            data: SyncUnsafeCell::new(data),
        }
    }

    pub fn thread_init(&self, weight: i32) {
        unsafe {
            fairlock_thread_init(self.lock.get(), weight);
        }
    }

    pub fn lock(&self) -> SCL_Guard<T> {
        unsafe {
            fairlock_acquire(self.lock.get());
            SCL_Guard::new(self)
        }
    }

    pub fn unlock(&self) {
        unsafe {
            fairlock_release(self.lock.get());
        }
    }
}

impl<T> DLock<T> for USCL<T> {
    fn lock<'a>(&self, mut f: impl DLockDelegate<T> + 'a) {
        let mut guard = self.lock();
        let data = &mut (*guard) as *mut T as *const SyncUnsafeCell<T>;
        unsafe {
            f.apply(DLockGuard::new(&*data));
        }
    }

    #[cfg(feature = "combiner_stat")]
    fn get_current_thread_combining_time(&self) -> Option<std::num::NonZeroI64> {
        None
    }
}
