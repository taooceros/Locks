use std::{
    fs::File,
    marker::PhantomData,
    mem::take,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle, Scope, ScopedJoinHandle},
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
            let writer =
                &mut Writer::from_path(self.output_path.join(format!("{}.csv", experiment)))
                    .unwrap();

            let job = match experiment {
                Experiment::RatioOneThree => one_three_benchmark,
                Experiment::Subversion => subversion_benchmark,
            };

            let targets = extract_targets(self.waiter, self.target);

            for target in targets {
                if let Some(lock) = target {
                    self.inner_benchmark(lock.into(), experiment, writer, job);
                }
            }

            if matches!(
                self.target,
                Some(LockTarget::DLock(DLockTarget::RCL)) | None
            ) {
                match self.waiter {
                    WaiterType::Spin => self.bench_rcl::<_, SpinParker>(experiment, writer, job),
                    WaiterType::Block => self.bench_rcl::<_, BlockParker>(experiment, writer, job),
                    WaiterType::All => {
                        self.bench_rcl::<_, SpinParker>(experiment, writer, job);
                        self.bench_rcl::<_, BlockParker>(experiment, writer, job)
                    }
                }
            }
            println!("{:?} finished", experiment);
        }
    }

    fn bench_rcl<R: Serialize, P>(
        &self,
        experiment: Experiment,
        writer: &mut Writer<File>,
        job: fn(LockBenchInfo<u64>) -> R,
    ) where
        P: Parker + 'static,
        R: Serialize + Send + Sync + 'static,
        BenchmarkType<u64>: From<DLockType<u64, P>>,
    {
        let mut server = RclServer::new();
        server.start(self.num_cpu - 1);
        let lock = RclLock::<u64, P>::new(&mut server, 0u64);

        self.inner_benchmark(
            Arc::new(DLockType::RCL(lock).into()),
            experiment,
            writer,
            job,
        );
    }

    fn inner_benchmark<T: Send + Sync + 'static, R: Serialize + Send + Sync + 'static>(
        &self,
        lock_type: Arc<BenchmarkType<T>>,
        experiment: Experiment,
        writer: &mut Writer<File>,
        job: fn(LockBenchInfo<T>) -> R,
    ) where
        R: Serialize,
    {
        static STOP: AtomicBool = AtomicBool::new(false);

        STOP.store(false, Ordering::Release);

        thread::scope(|s| {
            let mut results: Vec<R> = Vec::new();

            {
                let mut threads = (0..self.num_thread)
                    .map(|id| BenchJob::new(self, lock_type.clone(), &STOP, id, s))
                    .collect::<Vec<_>>();

                println!("Starting {} for {}|{}", experiment, lock_type.lock_name(), lock_type.parker_name());

                for thread in threads.iter_mut() {
                    thread.start_benchmark(job);
                }

                thread::sleep(Duration::from_secs(self.duration));

                STOP.store(true, Ordering::Release);

                let mut i = 0;

                for job in threads.iter_mut() {
                    let l = std::mem::take(&mut job.handle)
                        .expect("Unexpected None Handle")
                        .join();
                    match l {
                        Ok(l) => {
                            results.push(l);
                            // println!("{}", l);
                        }
                        Err(_e) => eprintln!("Error joining thread: {}", i),
                    }
                    i += 1;
                }
            }

            for result in results.iter() {
                writer.serialize(result).unwrap();
            }

            // let total_count: u64 = results.iter().map(|r| r.loop_count).sum();

            // lock_type.lock(|guard: DLockGuard<u64>| {
            //     assert_eq!(
            //         *guard, total_count,
            //         "Total counter is not matched with lock value {}, but thread local loop sum {}",
            //         *guard, total_count
            //     );
            // });

            // println!(
            //     "Finish Benchmark for {}: Total Counter {}",
            //     lock_type, total_count
            // );
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
    pub stop: &'a AtomicBool,
    pub id: usize,
}

pub struct BenchJob<'scope, 'env, T, R>
where
    T: Send + Sync + 'static,
    R: Serialize,
{
    info: Option<LockBenchInfo<'scope, T>>,
    handle: Option<ScopedJoinHandle<'scope, R>>,
    scope: &'scope Scope<'scope, 'env>,
    _photom_r: PhantomData<R>,
}

impl<'scope, 'env, T, R> BenchJob<'scope, 'env, T, R>
where
    T: Send + Sync + 'static,
    R: Serialize + Send + Sync + 'static,
{
    pub fn new(
        bencher: &'env Bencher,
        lock_type: Arc<BenchmarkType<T>>,
        stop: &'env AtomicBool,
        id: usize,
        scope: &'scope Scope<'scope, 'env>,
    ) -> BenchJob<'scope, 'env, T, R> {
        Self {
            info: Some(LockBenchInfo {
                lock_type,
                num_thread: bencher.num_thread,
                num_cpu: bencher.num_cpu,
                stop,
                id,
            }),
            handle: None,
            _photom_r: PhantomData,
            scope,
        }
    }

    fn start_benchmark<F>(&mut self, job: F)
    where
        F: FnOnce(LockBenchInfo<T>) -> R + Send + Sync + 'static,
    {
        let info = self.info.take().expect("Unexpected None Info");

        self.handle = Some(self.scope.spawn(move || job(info)));
    }
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
