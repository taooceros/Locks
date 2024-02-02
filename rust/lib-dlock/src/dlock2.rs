use std::marker::PhantomData;

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

pub trait DLock2Delegate<T> = Fn(&mut T, T) -> T + Send + Sync;

#[enum_dispatch(DLock2Impl<T, F>)]
pub trait DLock2<T, F>: Send + Sync
where
    F: DLock2Delegate<T>,
{
    fn lock(&self, data: T) -> T;
}

#[enum_dispatch]
#[derive(Debug, Display)]
pub enum DLock2Impl<T, F>
where
    T: Send + Sync,
    F: DLock2Delegate<T> + 'static,
{
    FC(FC<T, F, RawSpinLock>),
    FCBan(FCBan<T, F, RawSpinLock>),
    CC(CCSynch<T, F>),
    CCBan(CCBan<T, F>),
    SpinLock(DLock2SpinLock<T, F>),
    Mutex(DLock2Mutex<T, F>),
    USCL(DLock2USCL<T, F>),
}
