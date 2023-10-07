
use serde_with::DurationMilliSeconds;

use std::num::NonZeroI64;
use std::path::Path;
use std::{
    fs::File,
    sync::{atomic::*, Arc},
    thread::{self, JoinHandle},
    time::Duration,
};


use csv::Writer;
use itertools::Itertools;
use libdlock::{
    dlock::{BenchmarkType, DLock, DLockType},
    guard::DLockGuard,
    parker::{block_parker::BlockParker, spin_parker::SpinParker, Parker},
    rcl::{rcllock::RclLock, rclserver::RclServer},
};

use serde::Serialize;
use serde_with::serde_as;
use strum::IntoEnumIterator;

use crate::benchmark::counter_job::one_three_benchmark;
use crate::benchmark::subversion_job::subversion_benchmark;
use crate::command_parser::*;

use self::bencher::Bencher;

mod counter_job;
mod subversion_job;
mod bencher;

pub fn benchmark(
    num_cpu: usize,
    num_thread: usize,
    experiment: Option<Experiment>,
    target: Option<LockTarget>,
    output_path: &Path,
    waiter: WaiterType,
    duration: u64,
) {
    let bencher = Bencher::new(
        num_cpu,
        num_thread,
        experiment,
        target,
        output_path.to_path_buf().into_boxed_path(),
        waiter,
        duration,
    );

    bencher.benchmark();
}




#[serde_as]
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct Record {
    id: usize,
    cpu_id: usize,
    thread_num: usize,
    cpu_num: usize,
    loop_count: u64,
    num_acquire: u64,
    #[serde_as(as = "DurationMilliSeconds<u64>")]
    hold_time: Duration,
    #[cfg(feature = "combiner_stat")]
    combine_time: Option<NonZeroI64>,
    locktype: String,
    waiter_type: String,
}


