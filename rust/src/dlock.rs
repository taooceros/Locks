use std::{fmt, sync::Mutex};

use enum_dispatch::enum_dispatch;

use crate::{ccsynch::CCSynch, flatcombining::FcLock, guard::DLockGuard, rcl::rcllock::RclLock};

impl<T, F> DLockDelegate<T> for F
where
    F: FnMut(DLockGuard<T>),
{
    fn apply(&mut self, data: DLockGuard<T>) {
        self(data);
    }
}

pub trait DLockDelegate<T> {
    fn apply(&mut self, data: DLockGuard<T>);
}

#[enum_dispatch]
pub trait DLock<T> {
    fn lock<'a>(&self, f: impl DLockDelegate<T> + 'a);
}

#[enum_dispatch(DLock<T>)]
pub enum LockType<T> {
    FlatCombining(FcLock<T>),
    CCSynch(CCSynch<T>),
    Mutex(Mutex<T>),
    RCL(RclLock<T>),
}

impl<T> fmt::Display for LockType<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FlatCombining(_) => write!(f, "Flat Combining"),
            Self::Mutex(_) => write!(f, "Mutex"),
            Self::CCSynch(_) => write!(f, "CCSynch"),
            Self::RCL(_) => write!(f, "RCL"),
        }
    }
}
