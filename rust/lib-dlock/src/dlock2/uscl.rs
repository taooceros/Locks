use std::ops::DerefMut;

use crate::u_scl::USCL;

use super::{DLock2, DLock2Delegate};

#[derive(Debug)]
pub struct DLock2USCL<T, F>
where
    F: Fn(&mut T, T) -> T + Send + Sync,
{
    delegate: F,
    data: USCL<T>,
}

impl<T, F> DLock2USCL<T, F>
where
    F: Fn(&mut T, T) -> T + Send + Sync,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            delegate,
            data: USCL::new(data),
        }
    }
}

impl<T, F> DLock2<T, F> for DLock2USCL<T, F>
where
    T: Send + Sync,
    F: DLock2Delegate<T>,
{
    fn lock(&self, data: T) -> T {
        let mut lock_data = self.data.lock();
        (self.delegate)(lock_data.deref_mut(), data)
    }
}
