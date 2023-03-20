use std::fmt::DebugStruct;

use super::*;

pub struct RclLockPtr<T: Sized> {
    pub lock: *const RclLock<T>,
}

unsafe impl<T: Sized> Send for RclLockPtr<T> {}
unsafe impl<T: Sized> Sync for RclLockPtr<T> {}

impl<T> From<*const RclLock<T>> for RclLockPtr<T> {
    fn from(lock: *const RclLock<T>) -> RclLockPtr<T> {
        RclLockPtr { lock }
    }
}

impl<T> Into<*const RclLock<T>> for RclLockPtr<T> {
    fn into(self) -> *const RclLock<T> {
        self.lock
    }
}

impl<T> From<&RclLock<T>> for RclLockPtr<T> {
    fn from(lock: &RclLock<T>) -> RclLockPtr<T> {
        RclLockPtr {
            lock: lock as *const RclLock<T>,
        }
    }
}



// implement Deref and DerefMut for RclLockPtr<T> to get the lock

impl<T> Deref for RclLockPtr<T> {
    type Target = RclLock<T>;

    fn deref(&self) -> &RclLock<T> {
        unsafe { &*self.lock }
    }
}

