use std::{
    fmt::{self, Debug, Display},
    sync::Mutex,
};

use enum_dispatch::enum_dispatch;

use crate::{
    ccsynch::CCSynch, flatcombining::fclock::FcLock, flatcombining2::FcLock2,
    flatcombining_fair_ban::FcFairBanLock, guard::DLockGuard, raw_spin_lock::RawSpinLock,
    rcl::rcllock::RclLock,
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

impl<T> serde::Serialize for LockType<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<T> Debug for LockType<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FlatCombining(arg0) => f.debug_tuple("FlatCombining").finish(),
            Self::FlatCombining2(arg0) => f.debug_tuple("FlatCombining2").finish(),
            Self::FlatCombiningFair(arg0) => f.debug_tuple("FlatCombiningFair").finish(),
            Self::CCSynch(arg0) => f.debug_tuple("CCSynch").finish(),
            Self::Mutex(arg0) => f.debug_tuple("Mutex").finish(),
            Self::RCL(arg0) => f.debug_tuple("RCL").finish(),
        }
    }
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
