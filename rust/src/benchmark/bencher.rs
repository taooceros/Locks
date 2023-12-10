use std::{path::Path, sync::Arc};

use itertools::Itertools;
use libdlock::{
    dlock::{BenchmarkType, DLockType},
    parker::{block_parker::BlockParker, spin_parker::SpinParker, Parker},
    rcl::{rcllock::RclLock, rclserver::RclServer},
};

use strum::IntoEnumIterator;

use crate::{
    benchmark::{
        non_cs_counter::counter_one_three_non_cs_one,
        one_three_ratio_counter::counter_one_three_benchmark,
        proposion_counter::counter_proportional,
        response_time_single_addition::benchmark_response_time_single_addition,
        response_time_variable::benchmark_response_time_one_three_ratio,
        subversion_job::counter_subversion_benchmark,
    },
    command_parser::{experiment::Experiment, lock_target::*},
};

pub struct Bencher<'a> {
    num_cpu: usize,
    num_thread: usize,
    experiment: Option<&'a Experiment>,
    targets: Vec<LockTarget>,
    output_path: Box<Path>,
    waiter: WaiterType,
    stat_response_time: bool,
    duration: u64,
    verbose: bool,
}

impl<'a> Bencher<'a> {
    pub fn new(
        num_cpu: usize,
        num_thread: usize,
        experiment: Option<&'a Experiment>,
        target: Vec<LockTarget>,
        output_path: Box<Path>,
        waiter: WaiterType,
        stat_response_time: bool,
        duration: u64,
        verbose: bool,
    ) -> Self {
        Self {
            num_cpu,
            num_thread,
            experiment,
            targets: target,
            output_path,
            waiter,
            stat_response_time,
            duration,
            verbose,
        }
    }

    pub fn benchmark(&self) {
        let experiments = match self.experiment.clone() {
            Some(e) => vec![e],
            None => Experiment::to_vec_ref(),
        };

        for experiment in experiments {
            let job = &match experiment {
                Experiment::CounterProportional {
                    cs_durations: cs_duration,
                    non_cs_durations: non_cs_duration,
                } => counter_proportional(cs_duration.clone(), non_cs_duration.clone()),
                Experiment::CounterRatioOneThree => counter_one_three_benchmark(),
                Experiment::CounterSubversion => to_dyn(counter_subversion_benchmark),
                Experiment::CounterRatioOneThreeNonCS => counter_one_three_non_cs_one(),
                Experiment::ResponseTimeSingleAddition => {
                    to_dyn(benchmark_response_time_single_addition)
                }
                Experiment::ResponseTimeRatioOneThree => {
                    to_dyn(benchmark_response_time_one_three_ratio)
                }
            };

            let targets = extract_targets(self.waiter, self.targets.iter());

            for target in targets {
                if let Some(lock) = target {
                    job(LockBenchInfo {
                        lock_type: Arc::new(lock),
                        num_thread: self.num_thread,
                        num_cpu: self.num_cpu,
                        experiment,
                        duration: self.duration,
                        stat_response_time: self.stat_response_time,
                        output_path: &self.output_path,
                        verbose: self.verbose,
                    });
                }
            }

            if self.targets.contains(&LockTarget::RCL) {
                match self.waiter {
                    WaiterType::Spin => {
                        self.bench_rcl::<_, SpinParker, _>(experiment, &self.output_path, job)
                    }
                    WaiterType::Block => {
                        self.bench_rcl::<_, BlockParker, _>(experiment, &self.output_path, job)
                    }
                    WaiterType::All => {
                        self.bench_rcl::<_, SpinParker, _>(experiment, &self.output_path, job);
                        self.bench_rcl::<_, BlockParker, _>(experiment, &self.output_path, job)
                    }
                }
            }
            println!("{:?} finished", experiment);
        }
    }

    fn bench_rcl<T, P, F>(&self, experiment: &Experiment, output_path: &Path, job: F)
    where
        T: Send + Sync + 'static + Default,
        P: Parker + 'static,
        BenchmarkType<T>: From<DLockType<T, P>>,
        F: Fn(LockBenchInfo<T>),
    {
        let mut server = RclServer::new();
        server.start(self.num_cpu - 1);
        let lock = DLockType::RCL(RclLock::<T, P>::new(&mut server, T::default()));

        job(LockBenchInfo {
            lock_type: Arc::new(lock.into()),
            num_thread: self.num_thread,
            num_cpu: self.num_cpu - 1,
            experiment,
            duration: self.duration,
            stat_response_time: self.stat_response_time,
            output_path,
            verbose: self.verbose,
        });
    }
}

pub struct LockBenchInfo<'a, T>
where
    T: Send + Sync + 'static,
{
    pub lock_type: Arc<BenchmarkType<T>>,
    pub num_thread: usize,
    pub num_cpu: usize,
    pub experiment: &'a Experiment,
    pub duration: u64,
    pub stat_response_time: bool,
    pub output_path: &'a Path,
    pub verbose: bool,
}

fn extract_targets<'a>(
    waiter: WaiterType,
    targets: impl Iterator<Item = &'a LockTarget>,
) -> Vec<Option<BenchmarkType<u64>>> {
    let targets: Vec<Option<BenchmarkType<u64>>> = match waiter {
        WaiterType::Spin => targets.map(|t| t.to_locktype::<SpinParker>()).collect(),
        WaiterType::Block => targets.map(|t| t.to_locktype::<BlockParker>()).collect(),
        WaiterType::All => targets
            .flat_map(|t| {
                if t.is_dlock() {
                    vec![
                        t.to_locktype::<SpinParker>(),
                        t.to_locktype::<BlockParker>(),
                    ]
                } else {
                    vec![t.to_locktype::<SpinParker>()]
                }
            })
            .collect(),
    };
    targets
}

pub fn to_dyn<'a, F>(f: F) -> Box<dyn Fn(LockBenchInfo<u64>) + 'a>
where
    F: Fn(LockBenchInfo<u64>) + 'a,
{
    Box::new(f)
}
