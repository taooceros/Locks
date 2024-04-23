use std::{
    cell::SyncUnsafeCell,
    ffi::c_void,
    mem::MaybeUninit,
    ptr::{self},
};

use crate::{dlock2::DLock2, dlock2::DLock2Delegate, fc_init, fc_lock, fc_lock_t};

#[derive(Debug)]
pub struct CFlatCombining<T, F, I>
where
    T: Sized,
    F: DLock2Delegate<T, I>,
    I: Send,
{
    job: F,
    data: SyncUnsafeCell<T>,
    lock: SyncUnsafeCell<fc_lock_t>,
    phantom: std::marker::PhantomData<I>,
}

unsafe impl<T, F, I> Sync for CFlatCombining<T, F, I>
where
    T: Sized,
    F: DLock2Delegate<T, I>,
    I: Send,
{
}

impl<T, F, I> CFlatCombining<T, F, I>
where
    T: Sized,
    F: DLock2Delegate<T, I>,
    I: Send,
{
    pub fn new(data: T, job: F) -> Self {
        unsafe {
            let flatcombining = CFlatCombining {
                job,
                data: data.into(),
                lock: MaybeUninit::zeroed().assume_init(),
                phantom: std::marker::PhantomData,
            };

            fc_init(flatcombining.lock.get());

            flatcombining
        }
    }
}
use crate::dlock2::CombinerSample;

unsafe impl<T, F, I> DLock2<I> for CFlatCombining<T, F, I>
where
    T: Sized + Send + Sync + 'static,
    F: DLock2Delegate<T, I> + 'static,
    I: Send + 'static,
{
    #[cfg(feature = "combiner_stat")]
    fn get_combine_stat(&self) -> Option<&CombinerSample> {
        None
    }

    fn lock(&self, input: I) -> I {
        unsafe {
            let mut wrapper = Wrapper { inner: self, input };

            let value = fc_lock(
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
    inner: &'a CFlatCombining<T, F, I>,
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
