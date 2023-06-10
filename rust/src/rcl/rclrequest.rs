use std::{cell::SyncUnsafeCell, fmt::Debug};

use crate::guard::DLockGuard;

use super::rcllock::*;

#[repr(C)]
pub struct RclRequest<T> {
    pub(super) real_me: usize,
    pub(super) lock: RclLockPtr<T>,
    pub(super) f: SyncUnsafeCell<Option<*mut (dyn FnMut(&mut DLockGuard<T>))>>,
}

unsafe impl<T> Send for RclRequest<T> {}
unsafe impl<T> Sync for RclRequest<T> {}

impl<T> Debug for RclRequest<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RclRequest")
            .field("real_me", &self.real_me)
            .field("lock", &(self.lock.lock as usize))
            .field("f", &(unsafe { *self.f.get() }).is_some())
            .finish()
    }
}

impl<T> RclRequest<T> {
    pub fn with_lock<'a>(lock: *const RclLock<T>) -> RclRequest<T> {
        RclRequest {
            real_me: 0,
            lock: lock.into(),
            f: SyncUnsafeCell::new(None),
        }
    }
}

#[repr(C)]
pub(super) struct RclRequestSized {
    s0: i32,
    s1: *const u8,
    s2: Option<*const u8>,
}

impl Default for RclRequestSized {
    fn default() -> Self {
        RclRequestSized {
            s0: 0,
            s1: std::ptr::null(),
            s2: None,
        }
    }
}

unsafe impl Send for RclRequestSized {}
unsafe impl Sync for RclRequestSized {}

pub trait RequestCallable: Sized {
    fn call(&mut self);
}

impl<T> RequestCallable for RclRequest<T> {
    fn call(&mut self) {
        let f_option = unsafe { &mut *self.f.get() };

        if let Some(f) = *f_option {
            if self.lock.lock.is_null() {
                panic!("lock is null");
            }

            let mut guard = DLockGuard::new(&(*self.lock).data);
            unsafe {
                (*f)(&mut guard);
            }
            *f_option = None;
        }
    }
}
