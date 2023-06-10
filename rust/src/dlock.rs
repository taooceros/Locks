use std::{fmt, sync::Mutex};

use enum_dispatch::enum_dispatch;

use crate::{ccsynch::CCSynch, flatcombining::FcLock, guard::DLockGuard, rcl::rcllock::RclLock};

#[enum_dispatch]
pub trait DLock<T> {
    fn lock<'b>(&self, f: &mut (dyn FnMut(&mut DLockGuard<T>) + 'b));
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
