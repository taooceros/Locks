use std::{
    fmt::{self, Debug},
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
    parker::{block_parker::BlockParker, spin_parker::SpinParker},
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
pub enum LockType<T: 'static> {
    FlatCombining(FcLock<T, RawSpinLock, BlockParker>),
    FlatCombiningFair(FcFairBanLock<T, RawSpinLock>),
    FlatCombiningFairSlice(FcFairBanSliceLock<T, RawSpinLock>),
    FlatCombiningFairSL(FcSL<T, RawSpinLock>),
    CCSynchSpin(CCSynch<T, SpinParker>),
    CCSynchBlock(CCSynch<T, BlockParker>),
    CCBanSpin(CCBan<T, SpinParker>),
    CCBanBlock(CCBan<T, BlockParker>),
    SpinLock(SpinLock<T>),
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
            Self::FlatCombining(_arg0) => f.debug_tuple("FlatCombining").finish(),
            Self::FlatCombiningFair(_arg0) => f.debug_tuple("FlatCombiningFair").finish(),
            Self::FlatCombiningFairSlice(_arg0) => f.debug_tuple("FlatCombiningFairSlice").finish(),
            Self::FlatCombiningFairSL(_arg0) => {
                f.debug_tuple("Flat Combining (Skip List)").finish()
            }
            Self::CCSynchSpin(_arg0) => f.debug_tuple("CCSynch/Spin").finish(),
            Self::CCSynchBlock(_arg0) => f.debug_tuple("CCSynch/Block").finish(),
            Self::CCBanSpin(_arg0) => f.debug_tuple("CCSynch (Ban)/Spin").finish(),
            Self::CCBanBlock(_arg0) => f.debug_tuple("CCSynch (Ban)/Block").finish(),
            Self::SpinLock(_arg0) => f.debug_tuple("SpinLock").finish(),
            Self::Mutex(_arg0) => f.debug_tuple("Mutex").finish(),
            Self::RCL(_arg0) => f.debug_tuple("RCL").finish(),
        }
    }
}

impl<T> fmt::Display for LockType<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FlatCombining(_) => write!(f, "Flat Combining"),
            Self::FlatCombiningFair(_) => write!(f, "Flat Combining Fair"),
            Self::FlatCombiningFairSlice(_) => write!(f, "Flat Combining Fair With Combiner Slice"),
            Self::FlatCombiningFairSL(_) => write!(f, "Flat Combining (SkipList)"),
            Self::SpinLock(_) => write!(f, "SpinLock"),
            Self::Mutex(_) => write!(f, "Mutex"),
            Self::CCSynchSpin(_) => write!(f, "CCSynch/Spin"),
            Self::CCSynchBlock(_) => write!(f, "CCSynch/Block"),
            Self::CCBanSpin(_) => write!(f, "CCSynch (Ban)/Spin"),
            Self::CCBanBlock(_) => write!(f, "CCSynch (Ban)/Block"),
            Self::RCL(_) => write!(f, "RCL"),
        }
    }
}
