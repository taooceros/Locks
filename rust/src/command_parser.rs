use std::{sync::Mutex, thread::available_parallelism};

use clap::*;
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
use strum::{Display, EnumIter, IntoEnumIterator};

#[derive(Debug, Parser)]
#[clap(name = "lock counter benchmark", version)]
/// Benchmark Utility
pub struct App {
    #[command(subcommand)]
    pub lock_target: Option<LockTarget>,
    #[command(flatten)]
    pub global_opts: GlobalOpts,
}

#[derive(Debug, Clone, Copy, Display, ValueEnum, EnumIter)]
pub enum Experiment {
    RatioOneThree,
    Subversion,
}

#[derive(Debug, Clone, Copy, ValueEnum, Display, Serialize)]
pub enum WaiterType {
    Spin,
    Block,
    All,
}

#[derive(Debug, Subcommand, EnumIter, Clone, Copy)]
pub enum DLockTarget {
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
}

#[derive(Debug, Subcommand, Clone, Copy)]
pub enum LockTarget {
    #[command(flatten)]
    DLock(DLockTarget),
    /// Benchmark Mutex
    Mutex,
    /// Benchmark Spinlock
    SpinLock,
    /// Benchmark U-SCL
    USCL,
}

pub enum LockTargetIterState {
    DLock(DLockTargetIter),
    Mutex,
    SpinLock,
    USCL,
    Stop,
}

pub struct LockTargetIter {
    state: LockTargetIterState,
}

impl Iterator for LockTargetIter {
    type Item = LockTarget;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.state {
            LockTargetIterState::DLock(iter) => {
                if let Some(dlock) = iter.next() {
                    return Some(LockTarget::DLock(dlock));
                } else {
                    self.state = LockTargetIterState::Mutex;
                    return self.next();
                }
            }
            LockTargetIterState::Mutex => {
                self.state = LockTargetIterState::SpinLock;
                return Some(LockTarget::Mutex);
            }
            LockTargetIterState::SpinLock => {
                self.state = LockTargetIterState::USCL;
                return Some(LockTarget::SpinLock);
            }
            LockTargetIterState::USCL => {
                self.state = LockTargetIterState::Stop;
                return Some(LockTarget::USCL);
            }
            LockTargetIterState::Stop => {
                self.state = LockTargetIterState::DLock(DLockTarget::iter());
                return None;
            }
        }
    }
}

impl IntoEnumIterator for LockTarget {
    type Iterator = LockTargetIter;

    fn iter() -> Self::Iterator {
        LockTargetIter {
            state: LockTargetIterState::DLock(DLockTarget::iter()),
        }
    }
}

impl LockTarget {
    pub fn to_locktype<P>(&self) -> Option<BenchmarkType<u64>>
    where
        P: Parker + 'static,
        BenchmarkType<u64>: From<DLockType<u64, P>>,
    {
        let locktype: DLockType<u64, P> = match self {
            LockTarget::DLock(DLockTarget::FcSL) => FcSL::new(0u64).into(),
            LockTarget::DLock(DLockTarget::FcLock) => FcLock::new(0u64).into(),
            LockTarget::DLock(DLockTarget::FcFairBanLock) => FcFairBanLock::new(0u64).into(),
            LockTarget::DLock(DLockTarget::FcFairBanSliceLock) => {
                FcFairBanSliceLock::new(0u64).into()
            }
            LockTarget::DLock(DLockTarget::CCSynch) => CCSynch::new(0u64).into(),
            LockTarget::DLock(DLockTarget::CCBan) => CCBan::new(0u64).into(),
            // RCL requires special treatment
            LockTarget::DLock(DLockTarget::RCL) => return None,
            LockTarget::SpinLock => {
                return Some(BenchmarkType::OtherLocks(SpinLock::new(0u64).into()))
            }
            LockTarget::Mutex => return Some(BenchmarkType::OtherLocks(Mutex::new(0u64).into())),
            LockTarget::USCL => return Some(BenchmarkType::OtherLocks(USCL::new(0u64).into())),
        };

        Some(locktype.into())
    }
}

#[derive(Debug, Args)]
pub struct GlobalOpts {
    #[arg(global = true, num_args(0..), value_delimiter = ',', value_terminator("."), long, short, default_values_t = [available_parallelism().unwrap().get()].to_vec())]
    pub threads: Vec<usize>,
    #[arg(global = true, num_args(0..), value_delimiter = ',', value_terminator("."), long, short, default_values_t = [available_parallelism().unwrap().get()].to_vec())]
    pub cpus: Vec<usize>,
    #[arg(global = true, long, short, default_value = "../visualization/output")]
    pub output_path: String,
    #[arg(global = true, long, short, default_value = "all")]
    pub waiter: WaiterType,
    #[arg(global = true, long, short, default_value = "5")]
    pub duration: u64,
    #[arg(global = true, long, short)]
    pub experiment: Option<Experiment>,
    #[arg(global = true, long, short)]
    pub verbose: bool,
}
