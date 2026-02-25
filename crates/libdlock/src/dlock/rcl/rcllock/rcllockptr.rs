use super::*;

#[repr(C)]
pub struct RclLockPtr<T, P>
where
    T: Sized,
    P: Parker + 'static,
{
    pub lock: *const RclLock<T, P>,
}

unsafe impl<T: Sized, P: Parker> Send for RclLockPtr<T, P> {}
unsafe impl<T: Sized, P: Parker> Sync for RclLockPtr<T, P> {}

impl<T, P: Parker> From<*const RclLock<T, P>> for RclLockPtr<T, P> {
    fn from(lock: *const RclLock<T, P>) -> RclLockPtr<T, P> {
        RclLockPtr { lock }
    }
}

impl<T, P: Parker> Into<*const RclLock<T, P>> for RclLockPtr<T, P> {
    fn into(self) -> *const RclLock<T, P> {
        self.lock
    }
}

impl<T, P: Parker> From<&RclLock<T, P>> for RclLockPtr<T, P> {
    fn from(lock: &RclLock<T, P>) -> RclLockPtr<T, P> {
        RclLockPtr {
            lock: lock as *const RclLock<T, P>,
        }
    }
}

// implement Deref and DerefMut for RclLockPtr<T, P> to get the lock

impl<T, P: Parker> Deref for RclLockPtr<T, P> {
    type Target = RclLock<T, P>;

    fn deref(&self) -> &RclLock<T, P> {
        unsafe { &*self.lock }
    }
}
