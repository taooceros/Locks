use std::sync::Mutex;

use clap::ValueEnum;
use libdlock::{
    ccsynch::CCSynch,
    ccsynch_fair_ban::CCBan,
    dlock::{BenchmarkType, DLockType},
    fc::fclock::FcLock,
    fc_fair_ban::FcFairBanLock,
    fc_fair_ban_slice::FcFairBanSliceLock,
    fc_sl::FCSL,
    fc_sl_naive::FCSLNaive,
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
    /// Benchmark Flat-Combining Skiplist Naive
    FCSLNaive,
    /// Benchmark Flat-Combining Skiplist
    FCSL,
    /// Benchmark Flat-Combining Lock
    FC,
    /// Benchmark Flat-Combining Fair (Banning) Lock
    FCBan,
    /// Benchmark Flat-Combining Fair (Banning & Combiner Slice) Lock
    FCBanSlice,

    /// Benchmark CCSynch
    CC,
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
            LockTarget::FCSLNaive
            | LockTarget::FCSL
            | LockTarget::FC
            | LockTarget::FCBan
            | LockTarget::FCBanSlice
            | LockTarget::CC
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
            LockTarget::FCSLNaive => FCSLNaive::new(0u64).into(),
            LockTarget::FCSL => FCSL::new(0u64).into(),
            LockTarget::FC => FcLock::new(0u64).into(),
            LockTarget::FCBan => FcFairBanLock::new(0u64).into(),
            LockTarget::FCBanSlice => FcFairBanSliceLock::new(0u64).into(),
            LockTarget::CC => CCSynch::new(0u64).into(),
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
