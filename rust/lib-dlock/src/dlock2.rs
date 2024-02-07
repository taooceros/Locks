use crate::{
    dlock2::{cc::CCSynch, fc::FC},
    spin_lock::RawSpinLock,
};
use enum_dispatch::enum_dispatch;
use strum::Display;

use self::{
    cc_ban::CCBan, fc_ban::FCBan, mutex::DLock2Mutex, spinlock::DLock2SpinLock, uscl::DLock2USCL,
};

pub mod cc;
pub mod cc_ban;
pub mod fc;
pub mod fc_ban;
pub mod rcl;

pub mod mutex;
pub mod spinlock;
pub mod uscl;

pub trait DLock2Delegate<T, I> = Fn(&mut T, I) -> I + Send + Sync;

#[enum_dispatch(DLock2Impl<T, I, F>)]
pub trait DLock2<T, I>: Send + Sync {
    fn lock(&self, data: I) -> I;

    #[cfg(feature = "combiner_stat")]
    fn get_combine_time(&self) -> Option<u64>;
}

#[enum_dispatch]
#[derive(Debug, Display)]
pub enum DLock2Impl<T, I, F>
where
    T: Send + Sync,
    I: Send,
    F: DLock2Delegate<T, I> + 'static,
{
    FC(FC<T, I, F, RawSpinLock>),
    FCBan(FCBan<T, I, F, RawSpinLock>),
    CC(CCSynch<T, I, F>),
    CCBan(CCBan<T, I, F>),
    SpinLock(DLock2SpinLock<T, I, F>),
    Mutex(DLock2Mutex<T, I, F>),
    USCL(DLock2USCL<T, I, F>),
}
