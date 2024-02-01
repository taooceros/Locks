use std::{ops::DerefMut};

use crate::spin_lock::SpinLock;

use super::DLock2;

pub struct DLock2SpinLock<T, F>
where
    F: Fn(&mut T, T) -> T + Send + Sync,
{
    delegate: F,
    data: SpinLock<T>,
}

impl<T, F> DLock2SpinLock<T, F>
where
    F: Fn(&mut T, T) -> T + Send + Sync,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            delegate,
            data: SpinLock::new(data),
        }
    }
}

impl<T, F> DLock2<T, F> for DLock2SpinLock<T, F>
where
    T: Send + Copy,
    F: Fn(&mut T, T) -> T + Send + Sync,
{
    fn lock(&self, data: T) -> T {
        let mut lock_data = self.data.lock();
        (self.delegate)(lock_data.deref_mut(), data)
    }
}