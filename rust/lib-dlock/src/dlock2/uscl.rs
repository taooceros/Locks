use std::ops::DerefMut;

use crate::u_scl::USCL;

use super::{DLock2, DLock2Delegate};

#[derive(Debug)]
pub struct DLock2USCL<T, I, F>
where
    I: Send,
    F: DLock2Delegate<T, I>,
{
    delegate: F,
    data: USCL<T>,
    phantom: std::marker::PhantomData<fn() -> I>,
}

impl<T, I, F> DLock2USCL<T, I, F>
where
    I: Send,
    F: DLock2Delegate<T, I>,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            delegate,
            data: USCL::new(data),
            phantom: std::marker::PhantomData,
        }
    }
}

unsafe impl<T, I, F> DLock2<I> for DLock2USCL<T, I, F>
where
    T: Send + Sync,
    I: Send,
    F: DLock2Delegate<T, I>,
{
    fn lock(&self, data: I) -> I {
        let mut lock_data = self.data.lock();
        (self.delegate)(lock_data.deref_mut(), data)
    }

    #[cfg(feature = "combiner_stat")]
    fn get_combine_time(&self) -> Option<u64> {
        None
    }
}
