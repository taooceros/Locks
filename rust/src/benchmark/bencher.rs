use std::{path::Path, sync::Arc};

use libdlock::dlock::BenchmarkType;

use crate::{
    benchmark::dlock2::benchmark_dlock2,
    command_parser::experiment::Experiment,
    experiment::{DLock1Option, DLock2Option},
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
