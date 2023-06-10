use std::ops::{Deref, DerefMut};

use crate::guard::DLockGuard;

pub(crate) struct Operation<T> {
    pub f: *mut (dyn FnMut(&mut DLockGuard<T>)),
}

unsafe impl<T> Sync for Operation<T> {}
unsafe impl<T> Send for Operation<T> {}
