use std::{
    fs::File,
    marker::PhantomData,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, Scope, ScopedJoinHandle},
    time::Duration,
};

use csv::Writer;
use itertools::Itertools;
use libdlock::{
    dlock::{BenchmarkType, DLockType},
    parker::{block_parker::BlockParker, spin_parker::SpinParker, Parker},
    rcl::{rcllock::RclLock, rclserver::RclServer},
};
use serde::Serialize;
use strum::IntoEnumIterator;

use crate::{
    benchmark::{counter_job::one_three_benchmark, subversion_job::subversion_benchmark},
    command_parser::{DLockTarget, Experiment, LockTarget, WaiterType},
};

pub struct Bencher {
    num_cpu: usize,
    num_thread: usize,
    experiment: Option<Experiment>,
    target: Option<LockTarget>,
    output_path: Box<Path>,
    waiter: WaiterType,
    duration: u64,
}

impl Bencher {
    pub fn new(
        num_cpu: usize,
        num_thread: usize,
        experiment: Option<Experiment>,
        target: Option<LockTarget>,
        output_path: Box<Path>,
        waiter: WaiterType,
        duration: u64,
    ) -> Self {
        Self {
            num_cpu,
            num_thread,
            experiment,
            target,
            output_path,
            waiter,
            duration,
        }
    }

    pub fn benchmark(&self) {
        let experiments = match self.experiment {
            Some(e) => vec![e],
            None => Experiment::iter().collect(),
        };

        for experiment in experiments {
            let job = match experiment {
                Experiment::RatioOneThree => one_three_benchmark,
                Experiment::Subversion => subversion_benchmark,
            };

            let targets = extract_targets(self.waiter, self.target);

            for target in targets {
                if let Some(lock) = target {
                    job(LockBenchInfo {
                        lock_type: Arc::new(lock),
                        num_thread: self.num_thread,
                        num_cpu: self.num_cpu,
                        experiment,
                        duration: self.duration,
                        output_path: &self.output_path,
                    });
                }
            }

            if matches!(
                self.target,
                Some(LockTarget::DLock(DLockTarget::RCL)) | None
            ) {
                match self.waiter {
                    WaiterType::Spin => self.bench_rcl::<_, SpinParker>(experiment, &self.output_path, job),
                    WaiterType::Block => self.bench_rcl::<_, BlockParker>(experiment, &self.output_path, job),
                    WaiterType::All => {
                        self.bench_rcl::<_, SpinParker>(experiment, &self.output_path, job);
                        self.bench_rcl::<_, BlockParker>(experiment, &self.output_path, job)
                    }
                }
            }
            println!("{:?} finished", experiment);
        }
    }

    fn bench_rcl<T, P>(
        &self,
        experiment: Experiment,
        output_path: &Path,
        job: fn(LockBenchInfo<T>),
    ) where
        T: Send + Sync + 'static + Default,
        P: Parker + 'static,
        BenchmarkType<T>: From<DLockType<T, P>>
    {
        let mut server = RclServer::new();
        server.start(self.num_cpu - 1);
        let lock = DLockType::RCL(RclLock::<T, P>::new(&mut server, T::default()));

        job(LockBenchInfo {
            lock_type: Arc::new(lock.into()),
            num_thread: self.num_thread,
            num_cpu: self.num_cpu,
            experiment,
            duration: self.duration,
            output_path,
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
    pub experiment: Experiment,
    pub duration: u64,
    pub output_path: &'a Path,
}

fn extract_targets(
    waiter: WaiterType,
    target: Option<LockTarget>,
) -> Vec<Option<BenchmarkType<u64>>> {
    let targets: Vec<Option<BenchmarkType<u64>>> = match waiter {
        WaiterType::Spin => match target {
            Some(target) => vec![target.to_locktype::<SpinParker>()],
            None => (LockTarget::iter().map(|t| t.to_locktype::<SpinParker>())).collect(),
        },
        WaiterType::Block => match target {
            Some(target) => vec![target.to_locktype::<BlockParker>()],
            None => (LockTarget::iter().map(|t| t.to_locktype::<BlockParker>())).collect(),
        },
        WaiterType::All => match target {
            Some(target) => {
                let mut locks = vec![target.to_locktype::<SpinParker>()];
                if matches!(target, LockTarget::DLock(_)) {
                    locks.push(target.to_locktype::<BlockParker>());
                }
                locks
            }
            None => (LockTarget::iter().map(|t| t.to_locktype::<SpinParker>()))
                .interleave(
                    LockTarget::iter()
                        .filter(|x| matches!(x, LockTarget::DLock(_)))
                        .map(|t| t.to_locktype::<BlockParker>()),
                )
                .collect(),
        },
    };
    targets
}
