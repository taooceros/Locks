use std::{
    fmt::{self, Debug, Display},
    sync::Mutex, mem::MaybeUninit,
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
    parker::{block_parker::BlockParker, spin_parker::SpinParker, Parker},
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
pub enum BenchmarkType<T>
where
    T: 'static,
{
    SpinDLock(DLockType<T, SpinParker>),
    BlockDLock(DLockType<T, BlockParker>),
    OtherLocks(ThirdPartyLock<T>),
}

impl<T> BenchmarkType<T> {
    pub fn parker_name(&self) -> &str {
        match self {
            BenchmarkType::SpinDLock(_) => SpinParker::name(),
            BenchmarkType::BlockDLock(_) => BlockParker::name(),
            BenchmarkType::OtherLocks(_) => "",
        }
    }

    pub fn lock_name(&self) -> String {
        match self {
            BenchmarkType::SpinDLock(lock) => format!("{}", lock),
            BenchmarkType::BlockDLock(lock) => format!("{}", lock),
            BenchmarkType::OtherLocks(lock) => format!("{}", lock),
        }
    }
}

impl<T> Display for BenchmarkType<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BenchmarkType::OtherLocks(other_lock) => Display::fmt(other_lock, f),
            BenchmarkType::SpinDLock(spin_dlock) => {
                write!(f, "{}|{}", spin_dlock, SpinParker::name())
            }
            BenchmarkType::BlockDLock(block_dlock) => {
                write!(f, "{}|{}", block_dlock, BlockParker::name())
            }
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
    FlatCombiningFairSlice(FcFairBanSliceLock<T, RawSpinLock, P>),
    FlatCombiningFairSL(FcSL<T, RawSpinLock, P>),
    CCSynch(CCSynch<T, P>),
    CCBan(CCBan<T, P>),
    RCL(RclLock<T, P>),
}

impl<T, P: Parker> fmt::Display for DLockType<T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FlatCombining(_) => write!(f, "Flat Combining"),
            Self::FlatCombiningFair(_) => write!(f, "Flat Combining Fair"),
            Self::FlatCombiningFairSlice(_) => write!(f, "Flat Combining Fair With Combiner Slice"),
            Self::FlatCombiningFairSL(_) => write!(f, "Flat Combining (SkipList)"),
            Self::CCSynch(_) => write!(f, "CCSynch"),
            Self::CCBan(_) => write!(f, "CCSynch (Ban)"),
            Self::RCL(_) => write!(f, "RCL"),
        }
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