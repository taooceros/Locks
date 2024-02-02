use std::{path::Path, sync::Arc};

use libdlock::{
    dlock::{
        rcl::{rcllock::RclLock, rclserver::RclServer},
        *,
    },
    parker::{block_parker::BlockParker, spin_parker::SpinParker, Parker},
};

use crate::{
    benchmark::{
        bencher::{to_dyn, LockBenchInfo},
        dlock::{
            non_cs_counter::counter_one_three_non_cs_one,
            one_three_ratio_counter::counter_one_three_benchmark,
            proposion_counter::counter_proportional,
            response_time_single_addition::benchmark_response_time_single_addition,
            response_time_variable::benchmark_response_time_one_three_ratio,
            subversion_job::counter_subversion_benchmark,
        },
    },
    experiment::{DLock1Experiment, DLock1Option},
    lock_target::*,
};

use super::bencher::Bencher;

pub mod non_cs_counter;
pub mod one_three_ratio_counter;
pub mod proposion_counter;
pub mod response_time_single_addition;
pub mod response_time_variable;
pub mod subversion_job;

fn extract_targets<'a>(
    waiter: WaiterType,
    targets: impl Iterator<Item = &'a DLock1Target>,
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

pub fn benchmark_dlock1(bencher: &Bencher, option: &DLock1Option) {
    let experiment = &option.experiment;

    let experiments = match experiment {
        Some(e) => vec![e],
        None => DLock1Experiment::to_vec_ref(),
    };

    for experiment in experiments {
        let job = &match experiment {
            DLock1Experiment::CounterProportional {
                cs_durations,
                non_cs_durations,
                file_name,
            } => counter_proportional(cs_durations.clone(), non_cs_durations.clone(), file_name),
            DLock1Experiment::CounterRatioOneThree => counter_one_three_benchmark(),
            DLock1Experiment::CounterSubversion => to_dyn(counter_subversion_benchmark),
            DLock1Experiment::CounterRatioOneThreeNonCS => counter_one_three_non_cs_one(),
            DLock1Experiment::ResponseTimeSingleAddition => {
                to_dyn(benchmark_response_time_single_addition)
            }
            DLock1Experiment::ResponseTimeRatioOneThree => {
                to_dyn(benchmark_response_time_one_three_ratio)
            }
        };

        let targets = extract_targets(option.waiter, option.targets.iter());

        for target in targets {
            if let Some(lock) = target {
                job(LockBenchInfo {
                    lock_type: Arc::new(lock),
                    num_thread: bencher.num_thread,
                    num_cpu: bencher.num_cpu,
                    experiment_name: &experiment.to_string(),
                    duration: bencher.duration,
                    stat_response_time: bencher.stat_response_time,
                    output_path: &bencher.output_path,
                    verbose: bencher.verbose,
                });
            }
        }

        if option.targets.contains(&DLock1Target::RCL) {
            match option.waiter {
                WaiterType::Spin => {
                    bench_rcl::<_, SpinParker, _>(bencher, experiment, &bencher.output_path, job)
                }
                WaiterType::Block => {
                    bench_rcl::<_, BlockParker, _>(bencher, experiment, &bencher.output_path, job)
                }
                WaiterType::All => {
                    bench_rcl::<_, SpinParker, _>(bencher, experiment, &bencher.output_path, job);
                    bench_rcl::<_, BlockParker, _>(bencher, experiment, &bencher.output_path, job)
                }
            }
        }
        println!("{:?} finished", experiment);
    }
}

fn bench_rcl<T, P, F>(bencher: &Bencher, experiment: &DLock1Experiment, output_path: &Path, job: F)
where
    T: Send + Sync + 'static + Default,
    P: Parker + 'static,
    BenchmarkType<T>: From<DLockType<T, P>>,
    F: Fn(LockBenchInfo<T>),
{
    let mut server = RclServer::new();
    server.start(bencher.num_cpu - 1);
    let lock = DLockType::RCL(RclLock::<T, P>::new(&mut server, T::default()));

    job(LockBenchInfo {
        lock_type: Arc::new(lock.into()),
        num_thread: bencher.num_thread,
        num_cpu: bencher.num_cpu - 1,
        experiment_name: &experiment.to_string(),
        duration: bencher.duration,
        stat_response_time: bencher.stat_response_time,
        output_path,
        verbose: bencher.verbose,
    });
}
