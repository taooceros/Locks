use std::{
    collections::{BTreeSet, BinaryHeap},
    fmt::Debug,
    sync::Mutex,
};

use clap::ValueEnum;
use libdlock::{
    c_binding::{ccsynch::CCCSynch, flatcombining::CFlatCombining},
    dlock::{
        ccsynch::CCSynch, ccsynch_fair_ban::CCBan, fc::fclock::FcLock, fc_fair_ban::FcFairBanLock,
        fc_fair_ban_slice::FcFairBanSliceLock, fc_sl::FCSL, fc_sl_naive::FCSLNaive, BenchmarkType,
        DLockType,
    },
    dlock2::{
        self, fc::FC, fc_ban::FCBan, fc_pq::UsageNode, mutex::DLock2Mutex, spinlock::DLock2Wrapper,
        uscl::DLock2USCL, DLock2Delegate, DLock2Impl,
    },
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

impl Default for WaiterType {
    fn default() -> Self {
        WaiterType::All
    }
}

#[derive(Debug, ValueEnum, EnumIter, Clone, Copy, PartialEq, Display)]
pub enum DLock1Target {
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

impl DLock1Target {
    pub fn is_dlock(&self) -> bool {
        match self {
            DLock1Target::FCSLNaive
            | DLock1Target::FCSL
            | DLock1Target::FC
            | DLock1Target::FCBan
            | DLock1Target::FCBanSlice
            | DLock1Target::CC
            | DLock1Target::CCBan
            | DLock1Target::RCL => true,
            DLock1Target::Mutex | DLock1Target::SpinLock | DLock1Target::USCL => false,
        }
    }

    pub fn to_locktype<P>(&self) -> Option<BenchmarkType<u64>>
    where
        P: Parker + 'static,
        BenchmarkType<u64>: From<DLockType<u64, P>>,
    {
        let locktype: DLockType<u64, P> = match self {
            DLock1Target::FCSLNaive => FCSLNaive::new(0u64).into(),
            DLock1Target::FCSL => FCSL::new(0u64).into(),
            DLock1Target::FC => FcLock::new(0u64).into(),
            DLock1Target::FCBan => FcFairBanLock::new(0u64).into(),
            DLock1Target::FCBanSlice => FcFairBanSliceLock::new(0u64).into(),
            DLock1Target::CC => CCSynch::new(0u64).into(),
            DLock1Target::CCBan => CCBan::new(0u64).into(),
            // RCL requires special treatment
            DLock1Target::RCL => return None,
            DLock1Target::SpinLock => {
                return Some(BenchmarkType::OtherLocks(SpinLock::new(0u64).into()))
            }
            DLock1Target::Mutex => return Some(BenchmarkType::OtherLocks(Mutex::new(0u64).into())),
            DLock1Target::USCL => return Some(BenchmarkType::OtherLocks(USCL::new(0u64).into())),
        };

        Some(locktype.into())
    }
}

#[derive(Debug, ValueEnum, EnumIter, Clone, Copy, PartialEq, Display)]
pub enum DLock2Target {
    /// Benchmark Flat-Combining Lock
    FC,
    /// Benchmark Flat-Combining Fair (Banning) Lock
    FCBan,

    /// Benchmark CCSynch
    CC,
    /// Benchmark CCSynch (Ban)
    CCBan,
    /// Benchmark DSMSynch
    DSM,
    /// Benchmark FC-SL
    FcSL,
    /// Benchmark FC-PQ (BTree)
    FcPqBTree,
    /// Benchmark FC-PQ (BinaryHeap)
    FcPqBHeap,
    /// Benchmark Mutex
    Mutex,
    /// Benchmark Spinlock
    SpinLock,
    /// Benchmark U-SCL
    USCL,
    /// Benchmark Flat Combining (C)
    FcC,
    /// Benchmark CCSynch (C)
    CcC,
}

impl DLock2Target {
    pub fn is_dlock(&self) -> bool {
        match self {
            DLock2Target::FC
            | DLock2Target::FCBan
            | DLock2Target::CC
            | DLock2Target::CCBan
            | DLock2Target::DSM
            | DLock2Target::FcC
            | DLock2Target::CcC
            | DLock2Target::FcSL
            | DLock2Target::FcPqBHeap
            | DLock2Target::FcPqBTree => true,
            DLock2Target::Mutex | DLock2Target::SpinLock | DLock2Target::USCL => false,
        }
    }

    pub fn to_locktype<T, I, F>(&self, data: T, _: I, f: F) -> Option<DLock2Impl<T, I, F>>
    where
        T: Send + Sync,
        I: Send + Sync + Debug + 'static,
        F: DLock2Delegate<T, I>,
    {
        Some::<DLock2Impl<T, I, F>>(match self {
            DLock2Target::FC => FC::new(data, f).into(),
            DLock2Target::FCBan => FCBan::new(data, f).into(),
            DLock2Target::CC => dlock2::cc::CCSynch::new(data, f).into(),
            DLock2Target::CCBan => dlock2::cc_ban::CCBan::new(data, f).into(),
            DLock2Target::DSM => dlock2::dsm::DSMSynch::new(data, f).into(),
            DLock2Target::FcSL => dlock2::fc_sl::FCSL::new(data, f).into(),
            DLock2Target::FcPqBTree => {
                dlock2::fc_pq::FCPQ::<T, I, BTreeSet<_>, F>::new(data, f).into()
            }
            DLock2Target::FcPqBHeap => {
                dlock2::fc_pq::FCPQ::<T, I, BinaryHeap<_>, F>::new(data, f).into()
            }
            DLock2Target::SpinLock => DLock2Wrapper::new(data, f).into(),
            DLock2Target::Mutex => DLock2Mutex::new(data, f).into(),
            DLock2Target::USCL => DLock2USCL::new(data, f).into(),
            DLock2Target::FcC => CFlatCombining::new(data, f).into(),
            DLock2Target::CcC => CCCSynch::new(data, f).into(),
        })
    }
}
