use std::{ops::DerefMut, sync::Mutex};

use super::{DLock2, DLock2Delegate};

#[derive(Debug)]
pub struct DLock2Mutex<T, F>
where
    F: Fn(&mut T, T) -> T + Send + Sync,
{
    delegate: F,
    data: Mutex<T>,
}

impl<T, F> DLock2Mutex<T, F>
where
    F: Fn(&mut T, T) -> T + Send + Sync,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            delegate,
            data: Mutex::new(data),
        }
    }
}

impl<T, F> DLock2<T, F> for DLock2Mutex<T, F>
where
    T: Send + Sync,
    F: DLock2Delegate<T>,
{
    fn lock(&self, data: T) -> T {
        let mut lock_data = self.data.lock().unwrap();
        (self.delegate)(lock_data.deref_mut(), data)
    }
}
