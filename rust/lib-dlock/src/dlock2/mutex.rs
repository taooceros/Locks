use std::{ops::DerefMut, sync::Mutex};

use super::{DLock2, DLock2Delegate};

use super::combiner_stat::CombinerSample;
#[derive(Debug)]
pub struct DLock2Mutex<T, I, F>
where
    I: Send,
    F: DLock2Delegate<T, I>,
{
    delegate: F,
    data: Mutex<T>,
    phantom: std::marker::PhantomData<fn() -> I>,
}

impl<T, I, F> DLock2Mutex<T, I, F>
where
    I: Send,
    F: DLock2Delegate<T, I>,
{
    pub fn new(data: T, delegate: F) -> Self {
        Self {
            delegate,
            data: Mutex::new(data),
            phantom: std::marker::PhantomData,
        }
    }
}

unsafe impl<T, I, F> DLock2<I> for DLock2Mutex<T, I, F>
where
    T: Send + Sync,
    I: Send,
    F: DLock2Delegate<T, I>,
{
    fn lock(&self, data: I) -> I {
        let mut lock_data = self.data.lock().unwrap();
        (self.delegate)(lock_data.deref_mut(), data)
    }

    #[cfg(feature = "combiner_stat")]
    fn get_combine_stat(&self) -> Option<&CombinerSample> {
        None
    }
}
