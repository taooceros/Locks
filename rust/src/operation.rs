use std::ops::{Deref, DerefMut};

use crate::guard::Guard;

pub(crate) struct Operation<T> {
    pub f: Box<(dyn FnMut(&mut Guard<T>))>,
}

unsafe impl<T> Sync for Operation<T> {}
unsafe impl<T> Send for Operation<T> {}

impl<T> Deref for Operation<T> {
    type Target = Box<(dyn FnMut(&mut Guard<T>))>;

    fn deref(&self) -> &Self::Target {
        &self.f
    }
}

impl<T> DerefMut for Operation<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.f
    }
}
