use std::{
    cell::SyncUnsafeCell,
    ffi::c_void,
    mem::MaybeUninit,
    ptr::{self},
};

use crate::{
    cc_synch_init, cc_synch_lock, cc_synch_t,
    dlock2::{DLock2, DLock2Delegate},
};

#[derive(Debug)]
pub struct CCCSynch<T, F, I>
where
    T: Sized,
    F: DLock2Delegate<T, I>,
    I: Send,
{
    job: F,
    data: SyncUnsafeCell<T>,
    lock: SyncUnsafeCell<cc_synch_t>,
    phantom: std::marker::PhantomData<I>,
}

unsafe impl<T, F, I> Sync for CCCSynch<T, F, I>
where
    T: Sized,
    F: DLock2Delegate<T, I>,
    I: Send,
{
}

impl<T, F, I> CCCSynch<T, F, I>
where
    T: Sized,
    F: DLock2Delegate<T, I>,
    I: Send,
{
    pub fn new(data: T, job: F) -> Self {
        unsafe {
            let ccsynch = CCCSynch {
                job,
                data: data.into(),
                lock: MaybeUninit::zeroed().assume_init(),
                phantom: std::marker::PhantomData,
            };

            cc_synch_init(ccsynch.lock.get());

            ccsynch
        }
    }
}
use crate::dlock2::CombinerStatistics;

unsafe impl<T, F, I> DLock2<I> for CCCSynch<T, F, I>
where
    T: Sized + Send + Sync + 'static,
    F: DLock2Delegate<T, I> + 'static,
    I: Send + 'static,
{
    #[cfg(feature = "combiner_stat")]
    fn get_combine_stat(&self) -> Option<&CombinerStatistics> {
        None
    }

    fn lock(&self, input: I) -> I {
        unsafe {
            let mut wrapper = Wrapper { inner: self, input };

            let value = cc_synch_lock(
                self.lock.get(),
                Some(callback::<T, F, I>),
                &mut wrapper as *mut _ as *mut c_void,
            );

            if value.is_null() {
                panic!("fc_lock failed")
            }

            let result = value as *mut I;

            result.read()
        }
    }
}

pub struct Wrapper<'a, T, F, I>
where
    T: Sized,
    F: DLock2Delegate<T, I>,
    I: Send,
{
    inner: &'a CCCSynch<T, F, I>,
    input: I,
}

unsafe extern "C" fn callback<T, F, I>(wrapper: *mut c_void) -> *mut c_void
where
    T: Sized,
    F: DLock2Delegate<T, I>,
    I: Send,
{
    let wrapper = wrapper as *mut Wrapper<T, F, I>;

    let result = ((*wrapper).inner.job)(
        (*wrapper).inner.data.get().as_mut().unwrap(),
        ptr::read(&(*wrapper).input),
    );

    (*wrapper).input = result;

    wrapper as *mut c_void
}
