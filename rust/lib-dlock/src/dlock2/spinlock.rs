use std::{cell::SyncUnsafeCell, ops::DerefMut};

use lock_api::RawMutex;

use super::{DLock2, DLock2Delegate};

#[derive(Debug)]
pub struct DLock2Wrapper<T, I, F, L>
where
    F: DLock2Delegate<T, I>,
    L: RawMutex,
{
    delegate: F,
    data: SyncUnsafeCell<T>,
    lock: L,
    phantom: std::marker::PhantomData<fn() -> I>,
}

impl<T, I, F, L> DLock2Wrapper<T, I, F, L>
where
    F: DLock2Delegate<T, I>,
    L: RawMutex,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            delegate,
            data: data.into(),
            lock: L::INIT,
            phantom: std::marker::PhantomData,
        }
    }
}

unsafe impl<T, I, F, L> DLock2<I> for DLock2Wrapper<T, I, F, L>
where
    T: Send + Sync,
    I: Send,
    L: RawMutex + Send + Sync,
    F: DLock2Delegate<T, I>,
{
    fn lock(&self, data: I) -> I {
        let mut lock_data = self.lock.lock();
        (self.delegate)(unsafe { self.data.get().as_mut().unwrap_unchecked() }, data)
    }

    #[cfg(feature = "combiner_stat")]
    fn get_combine_time(&self) -> Option<u64> {
        return None;
    }
}
