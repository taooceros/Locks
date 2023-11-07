use serde_with::DurationMilliSeconds;

use std::num::NonZeroI64;
use std::path::{Path, PathBuf};
use std::thread::LocalKey;
use std::time::Duration;

use serde::Serialize;
use serde_with::serde_as;

use crate::command_parser::*;

use self::bencher::Bencher;

mod bencher;
mod counter_job;
mod helper;
mod subversion_job;
mod response_time;

pub fn benchmark(
    num_cpu: usize,
    num_thread: usize,
    lock_target: Option<LockTarget>,
    options: &GlobalOpts,
) {
    let bencher = Bencher::new(
        num_cpu,
        num_thread,
        options.experiment,
        lock_target,
        Path::new(&options.output_path)
            .to_path_buf()
            .into_boxed_path(),
        options.waiter,
        options.duration,
        options.verbose,
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
