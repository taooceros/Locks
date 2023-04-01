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

use quanta::Clock;

use crate::{
    ccsynch::CCSynch,
    flatcombining::{FcLock},
    rcl::{rcllock::RclLock, rclserver::RclServer}, dlock::{DLock, LockType},
};




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


    let mut server = RclServer::new(15);
    let lock = RclLock::new(&mut server, 0u64);
    inner_benchmark(Arc::new(LockType::RCL(lock)), output_path);

    println!("Benchmark finished");
}

fn inner_benchmark(lock_type: Arc<LockType<u64>>, output_path: &Path) {
    let num_cpus = num_cpus::get();
    println!("Number of cpus: {}", num_cpus);

    static STOP: AtomicBool = AtomicBool::new(false);

    STOP.store(false, Ordering::Release);

    let threads = (0..num_cpus)
        .map(|id| {
            return benchmark_num_threads(&lock_type, id, &STOP);
        })
        .collect::<Vec<_>>();

    println!("Starting benchmark for {}", lock_type);

    let mut results = Vec::new();

    thread::sleep(Duration::from_secs(3));

    STOP.store(true, Ordering::Release);

    let mut i = 0;

    for thread in threads {
        let l = thread.join();
        match l {
            Ok(l) => {
                results.push(l);
                // println!("{}", l);
            }
            Err(_e) => eprintln!("Error joining thread: {}", i),
        }
        i += 1;
    }

    let mut file = fs::File::create(output_path.join(format!("{}.csv", lock_type)))
        .ok()
        .unwrap();

    for result in results {
        file.write_fmt(format_args!("{}\n", result)).ok().unwrap();
    }
}

fn benchmark_num_threads(
    lock_type_ref: &Arc<LockType<u64>>,
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

            let mut loop_result = 0u64;

            match *lock_type {
                LockType::FlatCombining(ref fc_lock) => {
                    while !stop.load(Ordering::Acquire) {
                        fc_lock.lock(&mut |guard| {
                            let timer = Clock::new();
                            let begin = timer.now();

                            while timer.now().duration_since(begin) < single_iter_duration {
                                (**guard) += 1;
                                loop_result += 1;
                            }
                        });
                    }
                }
                LockType::Mutex(ref mutex) => {
                    while !stop.load(Ordering::Acquire) {
                        let timer = Clock::new();
                        let begin = timer.now();
                        let guard = mutex.lock();
                        if let Ok(mut guard) = guard {
                            while timer.now().duration_since(begin) < single_iter_duration {
                                *guard += 1;
                                loop_result += 1;
                            }
                        }
                        thread::sleep(Duration::from_nanos(1));
                    }
                }
                LockType::CCSynch(ref ccsynch) => {
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
                }
                LockType::RCL(ref rcl) => {
                    while !stop.load(Ordering::Acquire) {
                        // println!("job begin");

                        rcl.lock(&mut |guard| {
                            let timer = Clock::new();
                            let begin = timer.now();

                            while timer.now().duration_since(begin) < single_iter_duration {
                                (**guard) += 1;
                                loop_result += 1;
                            }
                        });

                        // println!("job end");
                    }
                }
            }

            println!("Thread {} finished with result {}", id, loop_result);

            return loop_result;
        })
        .unwrap()
}
