use std::{
    fmt,
    fs::{self, create_dir, remove_dir_all},
    io::Write,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use once_cell::sync::Lazy;
use quanta::{Clock};

use crate::{
    ccsynch::{CCSynch},
    flatcombining::{FCGuard, FcLock},
};

enum LockType {
    FlatCombining(FcLock<u64>),
    CCSynch(CCSynch<u64>),
    Mutex(Mutex<u64>),
}

unsafe impl Send for LockType {}
unsafe impl Sync for LockType {}

impl fmt::Display for LockType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FlatCombining(_) => write!(f, "Flat Combining"),
            Self::Mutex(_) => write!(f, "Mutex"),
            Self::CCSynch(_) => write!(f, "CCSynch"),
        }
    }
}

static FC_LOCK: Lazy<FcLock<u64>> = Lazy::new(|| FcLock::new(0u64));
static MUTEX_LOCK: Lazy<Mutex<u64>> = Lazy::new(|| Mutex::new(0u64));

pub fn benchmark() {
    let output_path = Path::new("output");

    if output_path.is_dir() {
        // remove the dir
        match remove_dir_all(output_path) {
            Ok(_) => {}
            Err(e) => {
                println!("Error removing output dir: {}", e);
                return;
            }
        }
    }

    match create_dir(output_path) {
        Ok(_) => {}
        Err(e) => {
            println!("Error creating output dir: {}", e);
            return;
        }
    }

    inner_benchmark(
        Arc::new(LockType::FlatCombining(FcLock::new(0u64))),
        output_path,
    );
    inner_benchmark(Arc::new(LockType::Mutex(Mutex::new(0u64))), output_path);
    inner_benchmark(Arc::new(LockType::CCSynch(CCSynch::new(0u64))), output_path);
}

fn inner_benchmark(lock_type: Arc<LockType>, output_path: &Path) {
    let num_cpus = num_cpus::get();
    static STOP: AtomicBool = AtomicBool::new(false);

    STOP.store(false, Ordering::Release);

    let threads = (1..num_cpus)
        .map(|id| {
            return benchmark_num_threads(&lock_type, id, &STOP);
        })
        .collect::<Vec<_>>();

    println!("Starting benchmark for {}", lock_type);

    let mut results = vec![];

    thread::sleep(Duration::from_secs(5));

    STOP.store(true, Ordering::Release);

    for thread in threads {
        let l = thread.join();
        if let Ok(l) = l {
            results.push(l);
        }
    }

    let mut file = fs::File::create(output_path.join(format!("{}.csv", lock_type)))
        .ok()
        .unwrap();

    for result in results {
        file.write_fmt(format_args!("{}\n", result)).ok();
    }
}

fn benchmark_num_threads(
    lock_type_ref: &Arc<LockType>,
    id: usize,
    stop: &'static AtomicBool,
) -> JoinHandle<u64> {
    let lock_type = lock_type_ref.clone();

    thread::Builder::new()
        .name(id.to_string())
        .spawn(move || {
            core_affinity::set_for_current(core_affinity::CoreId { id: id as usize });

            let single_iter_duration: Duration = Duration::from_micros({
                if id % 2 == 0 {
                    100
                } else {
                    300
                }
            });

            match *lock_type {
                LockType::FlatCombining(ref fc_lock) => {
                    let mut loop_result = 0u64;

                    while !stop.load(Ordering::Acquire) {
                        fc_lock.lock(&mut |guard: &mut FCGuard<u64>| {
                            let timer = Clock::new();
                            let begin = timer.now();

                            while timer.now().duration_since(begin) < single_iter_duration {
                                (**guard) += 1;
                                loop_result += 1;
                            }
                        });
                    }
                    return loop_result;
                }
                LockType::Mutex(ref mutex) => {
                    let mut result = 0u64;
                    while !stop.load(Ordering::Acquire) {
                        let timer = Clock::new();
                        let begin = timer.now();
                        let guard = mutex.lock();
                        if let Ok(mut guard) = guard {
                            while timer.now().duration_since(begin) < single_iter_duration {
                                *guard += 1;
                                result += 1;
                            }
                        }
                        thread::sleep(Duration::from_nanos(1));
                    }
                    return result;
                }
                LockType::CCSynch(ref ccsynch) => {
                    let mut loop_result = 0u64;

                    while !stop.load(Ordering::Acquire) {
                        ccsynch.lock(&mut |guard| {
                            let timer = Clock::new();
                            let begin = timer.now();

                            while timer.now().duration_since(begin) < single_iter_duration {
                                (**guard) += 1;
                                loop_result += 1;
                            }
                        });
                    }
                    return loop_result;
                }
            }
        })
        .unwrap()
}
