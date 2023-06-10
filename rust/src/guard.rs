use std::{
    cell::SyncUnsafeCell,
    ops::{Deref, DerefMut},
};

pub struct DLockGuard<'a, T: ?Sized> {
    data: &'a SyncUnsafeCell<T>,
}

impl<T: Sized> Deref for DLockGuard<'_, T> {
    fn deref(&self) -> &T {
        unsafe { &*self.data.get() }
    }

    type Target = T;
}

impl<T: Sized> DerefMut for DLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.data.get() }
    }
}

impl<T> DLockGuard<'_, T> {
    pub fn new<'a>(data: &'a SyncUnsafeCell<T>) -> DLockGuard<'a, T> {
        DLockGuard { data }
    }
}
