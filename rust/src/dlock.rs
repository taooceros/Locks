use std::{
    fmt::{self, Debug, Display},
    sync::Mutex,
};

use enum_dispatch::enum_dispatch;

use crate::{
    ccsynch::CCSynch,
    ccsynch_fair_ban::CCBan,
    fc::fclock::FcLock,
    fc_fair_ban::FcFairBanLock,
    fc_fair_ban_slice::FcFairBanSliceLock,
    fc_fair_skiplist::FcSL,
    guard::DLockGuard,
    parker::Parker,
    rcl::rcllock::RclLock,
    spin_lock::{RawSpinLock, SpinLock},
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

    #[cfg(feature = "combiner_stat")]
    fn get_current_thread_combining_time(&self) -> Option<std::num::NonZeroI64>;
}

#[enum_dispatch(DLock<T>)]
#[derive(Debug)]
pub enum ThirdPartyLock<T: 'static> {
    Mutex(Mutex<T>),
    SpinLock(SpinLock<T>),
}

#[enum_dispatch(DLock<T>)]
#[derive(Debug)]
pub enum BenchmarkType<T, P>
where
    T: 'static,
    P: Parker + 'static,
{
    DLock(DLockType<T, P>),
    OtherLocks(ThirdPartyLock<T>),
}

impl<T, P> Display for BenchmarkType<T, P>
where
    P: Parker,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BenchmarkType::DLock(dlock) => Display::fmt(&dlock, f),
            BenchmarkType::OtherLocks(other_lock) => Display::fmt(other_lock, f),
        }
    }
}

#[enum_dispatch(DLock<T>)]
#[derive(Debug)]
pub enum DLockType<T, P>
where
    T: 'static,
    P: Parker + 'static,
{
    FlatCombining(FcLock<T, RawSpinLock, P>),
    FlatCombiningFair(FcFairBanLock<T, RawSpinLock, P>),
    FlatCombiningFairSlice(FcFairBanSliceLock<T, RawSpinLock>),
    FlatCombiningFairSL(FcSL<T, RawSpinLock, P>),
    CCSynch(CCSynch<T, P>),
    CCBanSpin(CCBan<T, P>),
    RCL(RclLock<T>),
}

impl<T, P: Parker> fmt::Display for DLockType<T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FlatCombining(_) => write!(f, "Flat Combining"),
            Self::FlatCombiningFair(_) => write!(f, "Flat Combining Fair"),
            Self::FlatCombiningFairSlice(_) => write!(f, "Flat Combining Fair With Combiner Slice"),
            Self::FlatCombiningFairSL(_) => write!(f, "Flat Combining (SkipList)"),
            Self::CCSynch(_) => write!(f, "CCSynch"),
            Self::CCBanSpin(_) => write!(f, "CCSynch (Ban)/Spin"),
            Self::RCL(_) => write!(f, "RCL"),
        }
        .and(write!(f, "|"))
        .and(write!(f, "{}", P::name()))
    }
}

impl<T> fmt::Display for ThirdPartyLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThirdPartyLock::Mutex(_) => write!(f, "Mutex"),
            ThirdPartyLock::SpinLock(_) => write!(f, "SpinLock"),
        }
    }
}
