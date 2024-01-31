use std::{
    fmt::{self, Debug, Display},
    sync::Mutex,
};

pub mod ccsynch;
pub mod ccsynch_fair_ban;
pub mod fc;
pub mod fc_fair_ban;
pub mod fc_fair_ban_slice;
pub mod fc_sl_naive;
pub mod fc_sl;
pub mod guard;
pub mod rcl;

pub mod mutex_extension;


use enum_dispatch::enum_dispatch;

use self::{
    ccsynch::CCSynch,
    ccsynch_fair_ban::CCBan,
    fc::fclock::FcLock,
    fc_fair_ban::FcFairBanLock,
    fc_fair_ban_slice::FcFairBanSliceLock,
    fc_sl_naive::FCSLNaive,
    guard::DLockGuard,
    rcl::rcllock::RclLock,
    fc_sl::FCSL,
};

use crate::{
    parker::{block_parker::BlockParker, spin_parker::SpinParker, Parker},
    spin_lock::{RawSpinLock, SpinLock},
    u_scl::USCL,
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
    USCL(USCL<T>),
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
    FC(FcLock<T, RawSpinLock, P>),
    FCBan(FcFairBanLock<T, RawSpinLock, P>),
    FCBanSlice(FcFairBanSliceLock<T, RawSpinLock, P>),
    FCSLNaive(FCSLNaive<T, RawSpinLock, P>),
    FCSL(FCSL<T, RawSpinLock, P>),
    CCSynch(CCSynch<T, P>),
    CCBan(CCBan<T, P>),
    RCL(RclLock<T, P>),
}

impl<T, P: Parker> fmt::Display for DLockType<T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FC(_) => write!(f, "FC"),
            Self::FCBan(_) => write!(f, "FC-Ban"),
            Self::FCBanSlice(_) => write!(f, "FC-Ban-CSlice"),
            Self::FCSLNaive(_) => write!(f, "FC-SL Naive"),
            Self::FCSL(_) => write!(f, "FC-SL"),
            Self::CCSynch(_) => write!(f, "CC"),
            Self::CCBan(_) => write!(f, "CC-Ban"),
            Self::RCL(_) => write!(f, "RCL"),
        }
    }
}

impl<T> fmt::Display for ThirdPartyLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ThirdPartyLock::Mutex(_) => write!(f, "Mutex"),
            ThirdPartyLock::SpinLock(_) => write!(f, "SpinLock"),
            ThirdPartyLock::USCL(_) => write!(f, "U-SCL"),
        }
    }
}
