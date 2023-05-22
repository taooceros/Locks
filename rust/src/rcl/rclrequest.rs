use std::fmt::Debug;


use crate::guard::Guard;

use super::rcllock::*;

#[repr(C)]
pub struct RclRequest<T> {
    pub(super) real_me: usize,
    pub(super) lock: RclLockPtr<T>,
    pub(super) f: Option<*mut (dyn FnMut(&mut Guard<T>))>,
}

unsafe impl<T> Send for RclRequest<T> {}
unsafe impl<T> Sync for RclRequest<T> {}

impl<T> Debug for RclRequest<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RclRequest")
            .field("real_me", &self.real_me)
            .field("lock", &(self.lock.lock as usize))
            .field("f", &self.f.is_some())
            .finish()
    }
}

impl<T> RclRequest<T> {
    pub fn with_lock<'a>(lock: *const RclLock<T>) -> RclRequest<T> {
        RclRequest {
            real_me: 0,
            lock: lock.into(),
            f: None,
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
        if let Some(f) = self.f {
            if self.lock.lock.is_null() {
                panic!("lock is null");
            }

            let mut guard = Guard::new(&(*self.lock).data);
            unsafe {
                (*f)(&mut guard);
            }
            self.f = None;
        }
    }
}
