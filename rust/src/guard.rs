use std::{
    cell::SyncUnsafeCell,
    ops::{Deref, DerefMut},
};

pub struct Guard<'a, T> {
    data: &'a SyncUnsafeCell<T>,
}

impl<T: Sized> Deref for Guard<'_, T> {
    fn deref(&self) -> &T {
        unsafe { &*self.data.get() }
    }

    type Target = T;
}

impl<T: Sized> DerefMut for Guard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.data.get() }
    }
}

impl<T> Guard<'_, T> {
    pub fn new<'a>(data: &'a SyncUnsafeCell<T>) -> Guard<'a, T> {
        Guard { data }
    }
}
