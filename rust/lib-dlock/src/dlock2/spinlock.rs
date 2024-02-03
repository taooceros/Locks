use std::ops::DerefMut;

use crate::spin_lock::SpinLock;

use super::{DLock2, DLock2Delegate};

#[derive(Debug)]
pub struct DLock2SpinLock<T, I, F>
where
    F: DLock2Delegate<T, I>,
{
    delegate: F,
    data: SpinLock<T>,
    phantom: std::marker::PhantomData<fn() -> I>,
}

impl<T, I, F> DLock2SpinLock<T, I, F>
where
    F: DLock2Delegate<T, I>,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            delegate,
            data: SpinLock::new(data),
            phantom: std::marker::PhantomData,
        }
    }
}

impl<T, I, F> DLock2<T, I, F> for DLock2SpinLock<T, I, F>
where
    T: Send + Sync,
    F: DLock2Delegate<T, I>,
{
    fn lock(&self, data: I) -> I {
        let mut lock_data = self.data.lock();
        (self.delegate)(lock_data.deref_mut(), data)
    }

    #[cfg(feature = "combiner_stat")]
    fn get_combine_time(&self) -> Option<u64> {
        return None;
    }
}
