use std::{fmt, sync::Mutex};

use enum_dispatch::enum_dispatch;

use crate::{
    ccsynch::CCSynch, flatcombining::fclock::FcLock, flatcombining2::FcLock2, guard::DLockGuard,
    raw_spin_lock::RawSpinLock, rcl::rcllock::RclLock, flatcombining_fair_ban::FcFairBanLock,
};

impl<T, F> DLockDelegate<T> for F
where
    F: FnMut(DLockGuard<T>) + Send + Sync,
{
    fn apply(&mut self, data: DLockGuard<T>) {
        self(data);
    }
}

pub trait DLockDelegate<T>: Send + Sync {
    fn apply(&mut self, data: DLockGuard<T>);
}

#[enum_dispatch]
pub trait DLock<T> {
    fn lock<'a>(&self, f: impl DLockDelegate<T> + 'a);
}

#[enum_dispatch(DLock<T>)]
pub enum LockType<T> {
    FlatCombining(FcLock<T>),
    FlatCombining2(FcLock2<T, RawSpinLock>),
    FlatCombiningFair(FcFairBanLock<T, RawSpinLock>),
    CCSynch(CCSynch<T>),
    Mutex(Mutex<T>),
    RCL(RclLock<T>),
}

impl<T> fmt::Display for LockType<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FlatCombining(_) => write!(f, "Flat Combining"),
            Self::FlatCombining2(_) => write!(f, "Flat Combining2"),
            Self::FlatCombiningFair(_) => write!(f, "Flat Combining Fair"),
            Self::Mutex(_) => write!(f, "Mutex"),
            Self::CCSynch(_) => write!(f, "CCSynch"),
            Self::RCL(_) => write!(f, "RCL"),
        }
    }
}
