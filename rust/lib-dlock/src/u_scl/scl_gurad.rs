use std::ops::{Deref, DerefMut};

use super::USCL;

pub struct SCL_Guard<'a, T> {
    lock: &'a USCL<T>,
}

impl<T> SCL_Guard<'_, T> {
    pub(super) fn new(lock: &USCL<T>) -> SCL_Guard<T> {
        SCL_Guard { lock }
    }
}

impl<T> DerefMut for SCL_Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Deref for SCL_Guard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T> Drop for SCL_Guard<'a, T> {
    fn drop(&mut self) {
        unsafe {
            self.lock.unlock();
        }
    }
}
