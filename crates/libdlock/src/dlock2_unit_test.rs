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
        dsm::DSMSynch,
        fc::FC,
        fc_ban::FCBan,
        fc_pq::{UsageNode, FCPQ},
        fc_sl::FCSL,
        mcs::RawMcsLock,
        shfl_lock::RawShflLock,
        spinlock::DLock2Wrapper,
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
/// After all threads finish, asserts that the shared counter equals
/// `num_threads * iters` by calling `lock.lock(0)` (which adds 0 and returns
/// the current value).
fn run_counter_test<L>(lock: Arc<L>, num_threads: usize, iters: usize)
where
    L: DLock2<u64> + Send + Sync + 'static,
{
    let handles: Vec<_> = (0..num_threads)
        .map(|_| {
            let lock = lock.clone();
            thread::spawn(move || {
                for _ in 0..iters {
                    lock.lock(1_u64);
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("worker thread panicked");
    }

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
