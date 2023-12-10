use std::sync::Mutex;

use clap::ValueEnum;
use libdlock::{
    ccsynch::CCSynch,
    ccsynch_fair_ban::CCBan,
    dlock::{BenchmarkType, DLockType},
    fc::fclock::FcLock,
    fc_fair_ban::FcFairBanLock,
    fc_fair_ban_slice::FcFairBanSliceLock,
    fc_fair_skiplist::FcSL,
    parker::Parker,
    spin_lock::SpinLock,
    u_scl::USCL,
};
use serde::Serialize;
use strum::{Display, EnumIter};

#[derive(Debug, Clone, Copy, ValueEnum, Display, Serialize)]
pub enum WaiterType {
    Spin,
    Block,
    All,
}
#[derive(Debug, ValueEnum, EnumIter, Clone, Copy, PartialEq)]
pub enum LockTarget {
    /// Benchmark Flat-Combining Skiplist
    FcSL,
    /// Benchmark Flat-Combining Lock
    FcLock,
    /// Benchmark Flat-Combining Fair (Banning) Lock
    FcFairBanLock,
    /// Benchmark Flat-Combining Fair (Banning & Combiner Slice) Lock
    FcFairBanSliceLock,

    /// Benchmark CCSynch
    CCSynch,
    /// Benchmark CCSynch (Ban)
    CCBan,
    /// Benchmark Remote Core Locking
    RCL,
    /// Benchmark Mutex
    Mutex,
    /// Benchmark Spinlock
    SpinLock,
    /// Benchmark U-SCL
    USCL,
}

impl LockTarget {
    pub fn is_dlock(&self) -> bool {
        match self {
            LockTarget::FcSL
            | LockTarget::FcLock
            | LockTarget::FcFairBanLock
            | LockTarget::FcFairBanSliceLock
            | LockTarget::CCSynch
            | LockTarget::CCBan
            | LockTarget::RCL => true,
            LockTarget::Mutex | LockTarget::SpinLock | LockTarget::USCL => false,
        }
    }

    pub fn to_locktype<P>(&self) -> Option<BenchmarkType<u64>>
    where
        P: Parker + 'static,
        BenchmarkType<u64>: From<DLockType<u64, P>>,
    {
        let locktype: DLockType<u64, P> = match self {
            LockTarget::FcSL => FcSL::new(0u64).into(),
            LockTarget::FcLock => FcLock::new(0u64).into(),
            LockTarget::FcFairBanLock => FcFairBanLock::new(0u64).into(),
            LockTarget::FcFairBanSliceLock => FcFairBanSliceLock::new(0u64).into(),
            LockTarget::CCSynch => CCSynch::new(0u64).into(),
            LockTarget::CCBan => CCBan::new(0u64).into(),
            // RCL requires special treatment
            LockTarget::RCL => return None,
            LockTarget::SpinLock => {
                return Some(BenchmarkType::OtherLocks(SpinLock::new(0u64).into()))
            }
            LockTarget::Mutex => return Some(BenchmarkType::OtherLocks(Mutex::new(0u64).into())),
            LockTarget::USCL => return Some(BenchmarkType::OtherLocks(USCL::new(0u64).into())),
        };

        Some(locktype.into())
    }
}
