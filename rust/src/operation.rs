use std::ops::{Deref, DerefMut};

use crate::guard::Guard;

pub(crate) struct Operation<T> {
    pub f: *mut (dyn FnMut(&mut Guard<T>)),
}

unsafe impl<T> Sync for Operation<T> {}
unsafe impl<T> Send for Operation<T> {}
