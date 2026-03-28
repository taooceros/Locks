use std::{cell::SyncUnsafeCell, fmt::Debug};

use crossbeam::utils::CachePadded;

use crate::{dlock::guard::DLockGuard, dlock::DLockDelegate, parker::Parker};

use super::rcllock::*;

#[repr(C)]
pub struct RclRequest<T, P>
where
    P: Parker + 'static,
{
    pub(super) real_me: usize,
    pub(super) lock: RclLockPtr<T, P>,
    pub(super) parker: P,
    pub(super) f: CachePadded<SyncUnsafeCell<Option<*mut (dyn DLockDelegate<T>)>>>,
}

unsafe impl<T, P: Parker> Send for RclRequest<T, P> {}
unsafe impl<T, P: Parker> Sync for RclRequest<T, P> {}

impl<T, P: Parker> Debug for RclRequest<T, P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RclRequest")
            .field("real_me", &self.real_me)
            .field("lock", &(self.lock.lock as usize))
            .field("parker", &self.parker)
            .field("f", &(unsafe { *self.f.get() }).is_some())
            .finish()
    }
}

impl<T, P: Parker> RclRequest<T, P> {
    pub fn with_lock<'a>(lock: *const RclLock<T, P>) -> RclRequest<T, P> {
        RclRequest {
            real_me: 0,
            lock: lock.into(),
            parker: Default::default(),
            f: SyncUnsafeCell::new(None).into(),
        }
    }
}

pub trait RequestCallable: Sized {
    fn call(&mut self);
}

impl<T, P: Parker> RequestCallable for RclRequest<T, P> {
    fn call(&mut self) {
        let f = unsafe { &mut *self.f.get() }.expect("should contains a delegate when calling");

        if self.lock.lock.is_null() {
            panic!("lock is null");
        }

        unsafe {
            let guard = DLockGuard::new(&(*self.lock).data);
            (*f).apply(guard);
        }
    }
}
