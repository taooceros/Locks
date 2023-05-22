use std::{fmt, sync::Mutex};

use crate::{ccsynch::CCSynch, flatcombining::FcLock, guard::Guard, rcl::rcllock::RclLock};

pub trait DLock<T> {
    fn lock<'b>(&self, f: &mut (dyn FnMut(&mut Guard<T>) + 'b));
}

pub enum LockType<T> {
    FlatCombining(FcLock<T>),
    CCSynch(CCSynch<T>),
    Mutex(Mutex<T>),
    RCL(RclLock<T>),
}

impl<T> DLock<T> for LockType<T> {
    fn lock<'b>(&self, f: &mut (dyn FnMut(&mut Guard<T>) + 'b)) {
        let lock : &dyn DLock<T> = match self {
            LockType::FlatCombining(l) => l,
            LockType::CCSynch(l) => l,
            LockType::Mutex(l) => l,
            LockType::RCL(l) => l,
        };
        lock.lock(f)
    }
}

unsafe impl<T> Send for LockType<T> {}
unsafe impl<T> Sync for LockType<T> {}

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
