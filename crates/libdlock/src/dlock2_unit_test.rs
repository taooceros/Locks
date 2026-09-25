use std::{
    cmp::Reverse,
    collections::{BTreeSet, BinaryHeap},
    sync::{mpsc::channel, Arc},
    thread,
    time::Duration,
};

use crate::{
    dlock2::{
        c_aqs::RawCAqs,
        cc::CCSynch,
        cc_ban::CCBan,
        cfl::RawCflLock,
        clh::RawClhLock,
        dsm::DSMSynch,
        fc::FC,
        fc_ban::FCBan,
        fc_pq::{UsageNode, FCPQ},
        fc_sl::FCSL,
        mcs::RawMcsLock,
        pthread_mutex::DLock2PthreadMutex,
        shfl_lock::RawShflLock,
        spinlock::DLock2Wrapper,
        ticket::RawTicketLock,
        DLock2,
    },
    spin_lock::RawSpinLock,
};

/// Number of lock operations per thread in each test.
const ITERATIONS: usize = 1_000;

/// Delegate that increments a shared counter and returns its new value.
///
/// Using a named function (rather than a closure) gives us a stable, concrete
/// `fn` pointer type that satisfies the `DLock2Delegate` bound and can appear
/// in type aliases below.
fn counter_delegate(counter: &mut u64, input: u64) -> u64 {
    *counter += input;
    *counter
}

/// Concrete delegate type used throughout the tests.
type Delegate = fn(&mut u64, u64) -> u64;

// ---------------------------------------------------------------------------
// Helper: run the counter correctness test
// ---------------------------------------------------------------------------

/// Spawns `num_threads` threads, each calling `lock.lock(1)` `iters` times.
/// Every returned counter value must occur exactly once, not just the final
/// sum; a lost response or a duplicated execution is observable.
fn run_counter_test<L>(lock: Arc<L>, num_threads: usize, iters: usize)
where
    L: DLock2<u64> + Send + Sync + 'static,
{
    let handles: Vec<_> = (0..num_threads)
        .map(|_| {
            let lock = lock.clone();
            thread::spawn(move || (0..iters).map(|_| lock.lock(1_u64)).collect::<Vec<_>>())
        })
        .collect();

    let mut responses = Vec::with_capacity(num_threads * iters);
    for h in handles {
        responses.extend(h.join().expect("worker thread panicked"));
    }
    responses.sort_unstable();
    assert_eq!(
        responses,
        (1..=(num_threads * iters) as u64).collect::<Vec<_>>(),
        "missing or duplicated per-request counter response",
    );

    // Add 0 to read the final counter value without modifying it.
    let final_val = lock.lock(0_u64);
    assert_eq!(
        final_val,
        (num_threads * iters) as u64,
        "counter mismatch: expected {}, got {}",
        num_threads * iters,
        final_val,
    );
}

// ---------------------------------------------------------------------------
// Helper: panic if the test takes longer than `d`
// ---------------------------------------------------------------------------

fn panic_after<T, F>(d: Duration, f: F) -> T
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (done_tx, done_rx) = channel();
    let handle = thread::spawn(move || {
        let val = f();
        done_tx.send(()).expect("unable to send completion signal");
        val
    });

    match done_rx.recv_timeout(d) {
        Ok(_) => handle.join().expect("test thread panicked"),
        Err(_) => panic!("test timed out after {:?}", d),
    }
}

// ---------------------------------------------------------------------------
// Macro: generate 2-, 4-, and 8-thread tests for a given lock constructor
// ---------------------------------------------------------------------------

/// Generate a sub-module with three `#[test]` functions (2, 4, 8 threads) for
/// the DLock2 lock produced by `$ctor`.  `$ctor` is evaluated freshly inside
/// each test function, so each test gets an independent lock instance.
macro_rules! dlock2_counter_tests {
    ($mod_name:ident, $ctor:expr) => {
        mod $mod_name {
            use super::*;

            #[test]
            fn threads_2() {
                panic_after(Duration::from_secs(60), || {
                    run_counter_test(Arc::new($ctor), 2, ITERATIONS);
                });
            }

            #[test]
            fn threads_4() {
                panic_after(Duration::from_secs(60), || {
                    run_counter_test(Arc::new($ctor), 4, ITERATIONS);
                });
            }

            #[test]
            fn threads_8() {
                panic_after(Duration::from_secs(60), || {
                    run_counter_test(Arc::new($ctor), 8, ITERATIONS);
                });
            }
        }
    };
}

/// Like `dlock2_counter_tests!` but marks each test `#[serial_test::serial]`
/// so the three thread-count variants run one at a time, and uses a reduced
/// iteration count.  Use this for spin-heavy lock variants (e.g. FCSL) whose
/// combiner model performs poorly in CPU-overcommitted test environments: the
/// batching benefit is lost when worker threads are time-sliced and cannot
/// push their nodes before the combiner starts, causing severe throughput
/// degradation.  Running fewer iterations makes the test complete quickly
/// even under heavy scheduling pressure from the parallel test harness.
macro_rules! dlock2_counter_tests_serial {
    ($mod_name:ident, $ctor:expr) => {
        mod $mod_name {
            use super::*;

            /// Reduced iteration count for spin-heavy locks tested in
            /// parallel with other spinning tests.  50 ops per thread is
            /// sufficient to verify correctness while completing in well
            /// under 60 s even at 1000× scheduling slowdown.
            const SERIAL_ITERATIONS: usize = 50;

            #[test]
            #[serial_test::serial]
            fn threads_2() {
                panic_after(Duration::from_secs(60), || {
                    run_counter_test(Arc::new($ctor), 2, SERIAL_ITERATIONS);
                });
            }

            #[test]
            #[serial_test::serial]
            fn threads_4() {
                panic_after(Duration::from_secs(60), || {
                    run_counter_test(Arc::new($ctor), 4, SERIAL_ITERATIONS);
                });
            }

            #[test]
            #[serial_test::serial]
            fn threads_8() {
                panic_after(Duration::from_secs(60), || {
                    run_counter_test(Arc::new($ctor), 8, SERIAL_ITERATIONS);
                });
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Type aliases for verbose FCPQ instantiations
// ---------------------------------------------------------------------------

type FCPQBTree = FCPQ<u64, u64, BTreeSet<UsageNode<'static, u64>>, Delegate, RawSpinLock>;

type FCPQBHeap =
    FCPQ<u64, u64, BinaryHeap<Reverse<UsageNode<'static, u64>>>, Delegate, RawSpinLock>;

// ---------------------------------------------------------------------------
// Per-variant test modules
// ---------------------------------------------------------------------------

dlock2_counter_tests!(fc, FC::<u64, u64, Delegate>::new(0_u64, counter_delegate));

dlock2_counter_tests!(
    fc_ban,
    FCBan::<u64, u64, Delegate>::new(0_u64, counter_delegate)
);

dlock2_counter_tests!(
    cc,
    CCSynch::<u64, u64, Delegate>::new(0_u64, counter_delegate)
);

dlock2_counter_tests!(
    cc_ban,
    CCBan::<u64, u64, Delegate>::new(0_u64, counter_delegate)
);

dlock2_counter_tests!(
    dsm,
    DSMSynch::<u64, u64, Delegate>::new(0_u64, counter_delegate)
);

dlock2_counter_tests_serial!(
    fc_sl,
    FCSL::<u64, u64, Delegate>::new(0_u64, counter_delegate)
);

dlock2_counter_tests!(fc_pq_btree, FCPQBTree::new(0_u64, counter_delegate));

dlock2_counter_tests!(fc_pq_bheap, FCPQBHeap::new(0_u64, counter_delegate));

dlock2_counter_tests!(
    mcs,
    DLock2Wrapper::<u64, u64, Delegate, RawMcsLock>::new(0_u64, counter_delegate)
);

dlock2_counter_tests!(
    shfl_lock,
    DLock2Wrapper::<u64, u64, Delegate, RawShflLock>::new(0_u64, counter_delegate)
);

dlock2_counter_tests!(
    shfl_lock_c,
    DLock2Wrapper::<u64, u64, Delegate, RawCAqs>::new(0_u64, counter_delegate)
);

dlock2_counter_tests!(
    cfl,
    DLock2Wrapper::<u64, u64, Delegate, RawCflLock>::new(0_u64, counter_delegate)
);

dlock2_counter_tests!(
    ticket,
    DLock2Wrapper::<u64, u64, Delegate, RawTicketLock>::new(0_u64, counter_delegate)
);

dlock2_counter_tests!(
    clh,
    DLock2Wrapper::<u64, u64, Delegate, RawClhLock>::new(0_u64, counter_delegate)
);

dlock2_counter_tests!(
    pthread_mutex,
    DLock2PthreadMutex::<u64, u64, Delegate>::new(0_u64, counter_delegate)
);

// Reuse each worker's published node across many requests. The payload owns a
// non-Copy heap allocation and is dropped by the caller, never by a stale PQ
// entry or a stale copy left in a MaybeUninit slot.
mod ownership {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Barrier,
    };

    const WORKERS: usize = 3;
    const REQUESTS: usize = 24;

    #[derive(Debug)]
    struct Request {
        worker: usize,
        sequence: usize,
        bytes: Vec<u8>,
        executions: usize,
        drops: Arc<Vec<AtomicUsize>>,
    }

    impl Drop for Request {
        fn drop(&mut self) {
            self.drops[self.worker * REQUESTS + self.sequence].fetch_add(1, Ordering::SeqCst);
        }
    }

    #[derive(Debug)]
    struct IdentityState {
        seen: Arc<Vec<AtomicUsize>>,
    }

    fn identity_delegate(state: &mut IdentityState, mut request: Request) -> Request {
        let index = request.worker * REQUESTS + request.sequence;
        request.executions = state.seen[index].fetch_add(1, Ordering::SeqCst) + 1;
        request.bytes.push(0xa5);
        request
    }

    type IdentityDelegate = fn(&mut IdentityState, Request) -> Request;
    type IdentityBTree = FCPQ<
        IdentityState,
        Request,
        BTreeSet<UsageNode<'static, Request>>,
        IdentityDelegate,
        RawSpinLock,
    >;
    type IdentityBHeap = FCPQ<
        IdentityState,
        Request,
        BinaryHeap<Reverse<UsageNode<'static, Request>>>,
        IdentityDelegate,
        RawSpinLock,
    >;

    fn counters() -> Arc<Vec<AtomicUsize>> {
        Arc::new(
            (0..WORKERS * REQUESTS)
                .map(|_| AtomicUsize::new(0))
                .collect(),
        )
    }

    fn run<L>(lock: L, seen: Arc<Vec<AtomicUsize>>, drops: Arc<Vec<AtomicUsize>>)
    where
        L: DLock2<Request> + Send + Sync + 'static,
    {
        let lock = Arc::new(lock);
        let start = Arc::new(Barrier::new(WORKERS));
        let handles: Vec<_> = (0..WORKERS)
            .map(|worker| {
                let lock = lock.clone();
                let drops = drops.clone();
                let start = start.clone();
                thread::spawn(move || {
                    start.wait();
                    for sequence in 0..REQUESTS {
                        let request = Request {
                            worker,
                            sequence,
                            bytes: vec![worker as u8, sequence as u8],
                            executions: 0,
                            drops: drops.clone(),
                        };
                        let response = lock.lock(request);
                        assert_eq!((response.worker, response.sequence), (worker, sequence));
                        assert_eq!(response.bytes, [worker as u8, sequence as u8, 0xa5]);
                        assert_eq!(
                            response.executions, 1,
                            "request was executed more than once"
                        );
                    }
                    #[cfg(feature = "combiner_stat")]
                    assert!(lock.get_combine_time().is_some());
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("identity worker panicked");
        }
        // No queue/list entry may own a moved-out payload during quiescent drop.
        drop(lock);
        for index in 0..WORKERS * REQUESTS {
            assert_eq!(seen[index].load(Ordering::SeqCst), 1, "execution {index}");
            assert_eq!(drops[index].load(Ordering::SeqCst), 1, "drop {index}");
        }
    }

    #[test]
    fn fc_identity_drop() {
        let seen = counters();
        let drops = counters();
        run(
            FC::<IdentityState, Request, IdentityDelegate>::new(
                IdentityState { seen: seen.clone() },
                identity_delegate,
            ),
            seen,
            drops,
        );
    }

    #[test]
    fn pq_btree_identity_drop() {
        let seen = counters();
        let drops = counters();
        run(
            IdentityBTree::new(IdentityState { seen: seen.clone() }, identity_delegate),
            seen,
            drops,
        );
    }

    #[test]
    fn pq_bheap_identity_drop() {
        let seen = counters();
        let drops = counters();
        run(
            IdentityBHeap::new(IdentityState { seen: seen.clone() }, identity_delegate),
            seen,
            drops,
        );
    }
}

// Deliberately opt-in: exercise over-capacity admissions on an oversubscribed
// host under an external subprocess watchdog, never panic_after's detached
// worker timeout. Each wave exits completely before the next reuses TLS IDs.
mod admission_churn_stress {
    use super::*;
    use std::sync::Barrier;

    const REQUESTS_PER_WORKER: usize = 8;

    fn run<L>(lock: L, workers: usize)
    where
        L: DLock2<u64> + Send + Sync + 'static,
    {
        let lock = Arc::new(lock);
        let mut completed = 0_u64;
        for _wave in 0..2 {
            let start = Arc::new(Barrier::new(workers));
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    let lock = lock.clone();
                    let start = start.clone();
                    thread::spawn(move || {
                        start.wait();
                        (0..REQUESTS_PER_WORKER)
                            .map(|_| lock.lock(1))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            let mut returned = Vec::with_capacity(workers * REQUESTS_PER_WORKER);
            for handle in handles {
                returned.extend(handle.join().expect("admission worker panicked"));
            }
            returned.sort_unstable();
            let wave_end = completed + (workers * REQUESTS_PER_WORKER) as u64;
            assert_eq!(
                returned,
                ((completed + 1)..=wave_end).collect::<Vec<_>>(),
                "missing/duplicate response with {workers} workers",
            );
            completed = wave_end;
        }

        // With no competing workers each request forces a separate combining
        // pass. Traverse FC's cleanup age/period after the previous waves'
        // workers have exited, then drop the now-quiescent lock.
        for _ in 0..=500 {
            completed += 1;
            assert_eq!(lock.lock(1), completed);
        }
        drop(lock);
    }

    #[test]
    #[ignore = "run with an external process watchdog on an oversubscribed host"]
    fn fc_64_65_128_workers() {
        for workers in [64, 65, 128] {
            run(FC::<u64, u64, Delegate>::new(0, counter_delegate), workers);
        }
    }

    #[test]
    #[ignore = "run with an external process watchdog on an oversubscribed host"]
    fn pq_btree_64_65_128_workers() {
        for workers in [64, 65, 128] {
            run(FCPQBTree::new(0, counter_delegate), workers);
        }
    }

    #[test]
    #[ignore = "run with an external process watchdog on an oversubscribed host"]
    fn pq_bheap_64_65_128_workers() {
        for workers in [64, 65, 128] {
            run(FCPQBHeap::new(0, counter_delegate), workers);
        }
    }
}
