use std::{fmt::Display, path::Path, sync::Arc};

use itertools::Itertools;
use libdlock::{
    dlock::rcl::{rcllock::RclLock, rclserver::RclServer},
    dlock::{BenchmarkType, DLockType},
    parker::{block_parker::BlockParker, spin_parker::SpinParker, Parker},
};

use strum::IntoEnumIterator;

use crate::{
    benchmark::{
        dlock::{
            non_cs_counter::counter_one_three_non_cs_one,
            one_three_ratio_counter::counter_one_three_benchmark,
            proposion_counter::counter_proportional,
            response_time_single_addition::benchmark_response_time_single_addition,
            response_time_variable::benchmark_response_time_one_three_ratio,
            subversion_job::counter_subversion_benchmark,
        },
        dlock2::benchmark_dlock2,
    },
    command_parser::{experiment::Experiment, lock_target::*},
    experiment::{DLock1Experiment, DLock1Option, DLock2Experiment, DLock2Option},
};

use super::dlock::benchmark_dlock1;

#[derive(Debug)]
pub struct Bencher<'a> {
    pub num_cpu: usize,
    pub num_thread: usize,
    pub experiment: Option<&'a Experiment>,
    pub output_path: Box<Path>,
    pub stat_response_time: bool,
    pub duration: u64,
    pub verbose: bool,
}

impl<'a> Bencher<'a> {
    pub fn new(
        num_cpu: usize,
        num_thread: usize,
        experiment: Option<&'a Experiment>,
        output_path: Box<Path>,
        stat_response_time: bool,
        duration: u64,
        verbose: bool,
    ) -> Self {
        Self {
            num_cpu,
            num_thread,
            experiment,
            output_path,
            stat_response_time,
            duration,
            verbose,
        }
    }

    pub fn benchmark(&self) {
        match self.experiment {
            Some(Experiment::DLock1(dlock1_option)) => {
                self.benchmark_dlock1(dlock1_option);
            }
            Some(Experiment::DLock2(dlock2_option)) => self.benchmark_dlock2(dlock2_option),
            None => {
                self.benchmark_dlock2(&DLock2Option {
                    experiment: None,
                    lock_targets: None,
                });
            }
        }
    }

    fn benchmark_dlock2(&self, option: &DLock2Option) {
        benchmark_dlock2(self, option);
    }

    fn benchmark_dlock1(&self, option: &DLock1Option) {
        benchmark_dlock1(self, option);
    }
}

pub struct LockBenchInfo<'a, T>
where
    T: Send + Sync + 'static,
{
    pub lock_type: Arc<BenchmarkType<T>>,
    pub num_thread: usize,
    pub num_cpu: usize,
    pub experiment_name: &'a str,
    pub duration: u64,
    pub stat_response_time: bool,
    pub output_path: &'a Path,
    pub verbose: bool,
}

pub fn to_dyn<'a, F>(f: F) -> Box<dyn Fn(LockBenchInfo<u64>) + 'a>
where
    F: Fn(LockBenchInfo<u64>) + 'a,
{
    Box::new(f)
}
