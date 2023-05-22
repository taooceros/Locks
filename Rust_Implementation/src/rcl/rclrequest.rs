use std::fmt::Debug;

use super::rcllock::*;

#[repr(C)]
pub struct RclRequest<'a, T: 'a> {
    pub(super) real_me: usize,
    pub(super) lock: *const RclLock<T>,
    pub(super) f: Option<&'a mut (dyn FnMut(&mut RclGuard<T>))>,
}

impl<'a, T> Debug for RclRequest<'a, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RclRequest").field("real_me", &self.real_me).field("lock", &self.lock).field("f", &self.f.is_some()).finish()
    }
}


impl<T> RclRequest<'_, T> {
    pub fn empty<'a>(lock: &'a RclLock<T>) -> RclRequest<'a, T> {
        RclRequest {
            real_me: 0,
            lock,
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

impl<T> RequestCallable for RclRequest<'_, T> {
    fn call(&mut self) {
        if let Some(ref mut f) = self.f {
            let lock = unsafe { &*self.lock };
            let mut guard = RclGuard::new(lock);
            f(&mut guard);
            self.f = None;
        }
    }
}
