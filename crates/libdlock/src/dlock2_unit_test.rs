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

/// Number of lock operations per thread in each test. Scaled down under Miri,
/// whose virtual clock drives panic_after's 60 s watchdog; release unchanged.
const ITERATIONS: usize = if cfg!(miri) { 50 } else { 1_000 };

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

// E0(b) fast path (plan/2026-09-27/e0b-fcpq-fast-path.md). Compiled under
// every fcpq_* feature combination; without a fast-path feature the same
// schedules exercise only the slow path. Each request carries its issuer and
// draws a unique ticket, so a lost, duplicated or misdelivered request fails
// regardless of schedule, and panic_after turns a hang into a failure.
mod fc_pq_fast_path {
    use super::*;
    use crate::{
        dlock2::fc_pq::ENROLL_WINDOW_HOOK, sequential_priority_queue::SequentialPriorityQueue,
    };
    use std::{
        fmt::Debug,
        hint::spin_loop,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering::*},
            Barrier,
        },
    };

    const FAST_PATH: bool = cfg!(any(
        feature = "fcpq_fast_path",
        feature = "fcpq_fast_path_notime"
    ));
    const STAT: bool = cfg!(feature = "fcpq_fast_path_stat");

    /// Makes a CS keep the combiner lock until released.
    #[derive(Debug, Default)]
    struct Hold {
        entered: AtomicBool,
        released: AtomicBool,
    }

    impl Hold {
        fn enter_and_wait(&self) {
            self.entered.store(true, Release);
            while !self.released.load(Acquire) {
                spin_loop();
            }
        }

        fn wait_entered(&self) {
            while !self.entered.load(Acquire) {
                spin_loop();
            }
        }

        fn release(&self) {
            self.released.store(true, Release);
        }
    }

    #[derive(Debug)]
    struct Request {
        worker: usize,
        sequence: usize,
        ticket: u64,
        /// How often this (worker, sequence) had executed, this run included.
        executions: u32,
        hold: Option<Arc<Hold>>,
    }

    impl Request {
        fn new(worker: usize, sequence: usize) -> Self {
            Request {
                worker,
                sequence,
                ticket: 0,
                executions: 0,
                hold: None,
            }
        }
    }

    #[derive(Debug)]
    struct State {
        tickets: u64,
        seen: Vec<Vec<u32>>,
        executions: Arc<Vec<AtomicUsize>>,
    }

    fn serve(state: &mut State, mut request: Request) -> Request {
        if let Some(hold) = &request.hold {
            hold.enter_and_wait();
        }
        state.tickets += 1;
        request.ticket = state.tickets;
        // Rows grow on demand for workers of unbounded length (thread_churn).
        let row = &mut state.seen[request.worker];
        if row.len() <= request.sequence {
            row.resize(request.sequence + 1, 0);
        }
        let seen = &mut row[request.sequence];
        *seen += 1;
        request.executions = *seen;
        state.executions[request.worker].fetch_add(1, Relaxed);
        request
    }

    type Serve = fn(&mut State, Request) -> Request;
    type Lock<PQ> = FCPQ<State, Request, PQ, Serve, RawSpinLock>;
    type BTree = BTreeSet<UsageNode<'static, Request>>;
    type BHeap = BinaryHeap<Reverse<UsageNode<'static, Request>>>;
    trait Queue =
        SequentialPriorityQueue<UsageNode<'static, Request>> + Debug + Send + Sync + 'static;

    fn new_lock<PQ: Queue>(
        workers: usize,
        max_requests: usize,
    ) -> (Arc<Lock<PQ>>, Arc<Vec<AtomicUsize>>) {
        let executions = Arc::new(
            (0..workers)
                .map(|_| AtomicUsize::new(0))
                .collect::<Vec<_>>(),
        );
        let state = State {
            tickets: 0,
            seen: vec![vec![0; max_requests]; workers],
            executions: executions.clone(),
        };
        (Arc::new(Lock::<PQ>::new(state, serve)), executions)
    }

    fn set_enroll_hook(hook: impl Fn() + 'static) {
        ENROLL_WINDOW_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    }

    /// This thread's fast-path hits; 0 when the counter is compiled out.
    fn own_hits<PQ: Queue>(lock: &Lock<PQ>) -> u64 {
        #[cfg(feature = "fcpq_fast_path_stat")]
        return lock.get_fast_path_hits().unwrap_or(0);
        #[cfg(not(feature = "fcpq_fast_path_stat"))]
        {
            let _ = lock;
            0
        }
    }

    fn issue<PQ: Queue>(lock: &Lock<PQ>, worker: usize, sequence: usize) -> u64 {
        let response = lock.lock(Request::new(worker, sequence));
        assert_eq!(
            (response.worker, response.sequence),
            (worker, sequence),
            "response delivered to the wrong request",
        );
        assert_eq!(
            response.executions, 1,
            "request ({worker}, {sequence}) executed {} times, ticket {}",
            response.executions, response.ticket,
        );
        response.ticket
    }

    const WORKERS: usize = if cfg!(miri) { 3 } else { 8 };
    const ROUNDS: usize = if cfg!(miri) { 1 } else { 8 };
    const SOLO: usize = if cfg!(miri) { 20 } else { 2_000 };
    const BURST: usize = if cfg!(miri) { 10 } else { 1_000 };
    const HAMMER: usize = if cfg!(miri) { 40 } else { 4_000 };
    const TRICKLE: usize = if cfg!(miri) { 5 } else { 100 };
    const THINK_SPINS: usize = if cfg!(miri) { 10 } else { 500 };
    const WINDOW_SPINS: usize = if cfg!(miri) { 10 } else { 2_000 };

    /// Rounds of: one worker alone (fast path expected after at most one
    /// miss), all workers at once, then worker 0 hammering while the others
    /// trickle in. Odd workers stall on every enrollment between
    /// `active=true` and ring publication, so fast-path CSs run while they
    /// are invisible to the gate (plan D5).
    fn phased_stress<PQ: Queue>() {
        let (lock, executions) = new_lock::<PQ>(WORKERS, ROUNDS * (SOLO + BURST + HAMMER));
        let barrier = Arc::new(Barrier::new(WORKERS));
        let handles: Vec<_> = (0..WORKERS)
            .map(|worker| {
                let lock = lock.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    if worker % 2 == 1 {
                        set_enroll_hook(|| (0..WINDOW_SPINS).for_each(|_| spin_loop()));
                    }
                    let mut tickets = Vec::new();
                    let mut run = |count: usize, think: usize| {
                        for _ in 0..count {
                            tickets.push(issue(&lock, worker, tickets.len()));
                            (0..think).for_each(|_| spin_loop());
                        }
                    };
                    for round in 0..ROUNDS {
                        barrier.wait();
                        if worker == round % WORKERS {
                            let before = own_hits(&lock);
                            run(SOLO, 0);
                            let hits = own_hits(&lock) - before;
                            if STAT && FAST_PATH {
                                // One combine pass retires stale PQ nodes.
                                assert!(hits >= SOLO as u64 - 1, "solo hits {hits}/{SOLO}");
                            } else {
                                assert_eq!(hits, 0);
                            }
                        }
                        barrier.wait();
                        run(BURST, 0);
                        barrier.wait();
                        if worker == 0 {
                            run(HAMMER, 0);
                        } else {
                            run(TRICKLE, THINK_SPINS);
                        }
                    }
                    (tickets, own_hits(&lock))
                })
            })
            .collect();

        let mut tickets = Vec::new();
        let mut hits = 0;
        for (worker, handle) in handles.into_iter().enumerate() {
            let (worker_tickets, worker_hits) = handle.join().expect("stress worker panicked");
            assert_eq!(
                executions[worker].load(Relaxed),
                worker_tickets.len(),
                "worker {worker}: executions != requests",
            );
            tickets.extend(worker_tickets);
            hits += worker_hits;
        }
        let total = ROUNDS * (SOLO + WORKERS * BURST + HAMMER + (WORKERS - 1) * TRICKLE);
        tickets.sort_unstable();
        assert_eq!(
            tickets,
            (1..=total as u64).collect::<Vec<_>>(),
            "missing or duplicated execution",
        );
        #[cfg(feature = "fcpq_fast_path_stat")]
        assert_eq!(lock.fast_path_hits(), hits);
        if !FAST_PATH {
            assert_eq!(hits, 0);
        }
    }

    const WINDOW_REQUESTS: usize = if cfg!(miri) { 5 } else { 100 };

    /// Deterministic D5 window: while worker 1 is stalled between
    /// `active=true` and its ring publication, worker 0's requests take the
    /// fast path (they never enroll). Worker 1 is then served exactly once,
    /// after all of them, by its own retry: worker 0 is idle by then.
    fn enrollment_window<PQ: Queue>() {
        let (lock, executions) = new_lock::<PQ>(2, WINDOW_REQUESTS + 1);
        let first = Arc::new(Hold::default());
        let window = Arc::new(Hold::default());
        let hammer_done = Arc::new(AtomicBool::new(false));

        let hammer = {
            let (lock, first, hammer_done) = (lock.clone(), first.clone(), hammer_done.clone());
            thread::spawn(move || {
                let enrollments = Arc::new(AtomicUsize::new(0));
                let counter = enrollments.clone();
                set_enroll_hook(move || {
                    counter.fetch_add(1, Relaxed);
                });
                // Holds the combiner lock until worker 1 is in the window.
                let mut request = Request::new(0, 0);
                request.hold = Some(first);
                assert_eq!(lock.lock(request).ticket, 1);

                let (enrolled, hit) = (enrollments.load(Relaxed), own_hits(&lock));
                let tickets: Vec<u64> = (1..=WINDOW_REQUESTS)
                    .map(|sequence| issue(&lock, 0, sequence))
                    .collect();
                let window_enrollments = enrollments.load(Relaxed) - enrolled;
                let window_hits = own_hits(&lock) - hit;
                hammer_done.store(true, Release);
                (tickets, window_enrollments, window_hits)
            })
        };

        first.wait_entered();
        let enroller = {
            let (lock, window) = (lock.clone(), window.clone());
            thread::spawn(move || {
                set_enroll_hook(move || window.enter_and_wait());
                issue(&lock, 1, 0)
            })
        };
        // Worker 1 has stored active=true but reserved no ring ticket.
        window.wait_entered();
        first.release();
        while !hammer_done.load(Acquire) {
            spin_loop();
        }
        window.release();

        let (tickets, window_enrollments, window_hits) = hammer.join().expect("hammer panicked");
        let enroller_ticket = enroller.join().expect("enroller panicked");
        assert_eq!(
            tickets,
            (2..=WINDOW_REQUESTS as u64 + 1).collect::<Vec<_>>()
        );
        assert_eq!(enroller_ticket, WINDOW_REQUESTS as u64 + 2);
        assert_eq!(executions[0].load(Relaxed), WINDOW_REQUESTS + 1);
        assert_eq!(executions[1].load(Relaxed), 1);
        if FAST_PATH {
            assert_eq!(window_enrollments, 0, "window requests left the fast path");
        } else {
            assert_eq!(window_enrollments, WINDOW_REQUESTS);
        }
        if STAT {
            let expected = if FAST_PATH { WINDOW_REQUESTS as u64 } else { 0 };
            assert_eq!(window_hits, expected);
        }
    }

    /// One thread: every request, including the first, takes the fast path;
    /// a fresh node starts inactive, so it never enrolls.
    #[cfg(feature = "fcpq_fast_path_stat")]
    fn single_thread_hits<PQ: Queue>() {
        const REQUESTS: usize = if cfg!(miri) { 20 } else { 10_000 };
        let (lock, executions) = new_lock::<PQ>(1, REQUESTS);
        assert_eq!(lock.get_fast_path_hits(), None);
        for sequence in 0..REQUESTS {
            assert_eq!(issue(&lock, 0, sequence), sequence as u64 + 1);
        }
        let expected = if FAST_PATH { REQUESTS as u64 } else { 0 };
        assert_eq!(lock.get_fast_path_hits(), Some(expected));
        assert_eq!(lock.fast_path_hits(), expected);
        assert_eq!(executions[0].load(Relaxed), REQUESTS);
    }

    const CHURN_ROUNDS: usize = if cfg!(miri) { 3 } else { 2_000 };
    const CHURN_MAX_REQUESTS: usize = 40;
    const CHURN_THINK_SPINS: usize = if cfg!(miri) { 10 } else { 200 };

    /// Sets the flag when dropped, so a failing test does not leave its
    /// hammers spinning under the rest of the suite.
    struct StopOnDrop(Arc<AtomicBool>);

    impl Drop for StopOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Relaxed);
        }
    }

    /// Two lock instances. Long-lived hammers alternate between them while
    /// waves of short-lived threads each send 1..=40 requests to one lock
    /// and exit right after their last response, so thread_local hands
    /// nearly every churn thread (and each final probe thread) an exited
    /// thread's node, with its usage and hammer-served history. The node is
    /// normally already deactivated by then: a combiner that serves a node
    /// retires it in the same pass. Spinning threads alive at once never
    /// exceed the available parallelism (one CPU: no hammers); the per-queue
    /// tests are serial so they do not add up.
    fn thread_churn<PQ: Queue>() {
        let cpus = thread::available_parallelism().map_or(1, |n| n.get());
        let hammers = (cpus / 2).min(4);
        let batch = (cpus - hammers).clamp(1, 6);
        // Worker ids: hammers, then churn threads, then one probe per lock.
        let probe = hammers + CHURN_ROUNDS * batch;
        let locks: Arc<[_; 2]> =
            Arc::new([(); 2].map(|_| new_lock::<PQ>(probe + 1, CHURN_MAX_REQUESTS)));
        let stop = StopOnDrop(Arc::new(AtomicBool::new(false)));

        let hammer_handles: Vec<_> = (0..hammers)
            .map(|worker| {
                let (locks, stop) = (locks.clone(), stop.0.clone());
                thread::spawn(move || {
                    let mut tickets = [Vec::new(), Vec::new()];
                    let mut next = worker;
                    while !stop.load(Relaxed) {
                        let l = next % 2;
                        tickets[l].push(issue(&locks[l].0, worker, tickets[l].len()));
                        next += 1;
                    }
                    tickets
                })
            })
            .collect();

        // tickets[l][worker]: tickets returned to `worker` by lock `l`.
        let mut tickets = [vec![Vec::new(); probe + 1], vec![Vec::new(); probe + 1]];
        for round in 0..CHURN_ROUNDS {
            let wave: Vec<_> = (0..batch)
                .map(|k| {
                    let locks = locks.clone();
                    let worker = hammers + round * batch + k;
                    let l = (round + k) % 2;
                    let requests = 1 + (round * 7 + k * 3) % CHURN_MAX_REQUESTS;
                    let think = if k % 2 == 0 { CHURN_THINK_SPINS } else { 0 };
                    let handle = thread::spawn(move || {
                        (0..requests)
                            .map(|sequence| {
                                (0..think).for_each(|_| spin_loop());
                                issue(&locks[l].0, worker, sequence)
                            })
                            .collect::<Vec<_>>()
                    });
                    (l, worker, handle)
                })
                .collect();
            for (l, worker, handle) in wave {
                tickets[l][worker] = handle.join().expect("churn thread panicked");
            }
        }
        drop(stop);
        for (worker, handle) in hammer_handles.into_iter().enumerate() {
            let [t0, t1] = handle.join().expect("hammer panicked");
            (tickets[0][worker], tickets[1][worker]) = (t0, t1);
        }

        for l in 0..2 {
            let total: usize = tickets[l].iter().map(Vec::len).sum();
            // All issuers have exited, so a fresh thread may reuse one's node;
            // its combine pass would run any request still enrolled.
            let probe_ticket = {
                let locks = locks.clone();
                thread::spawn(move || issue(&locks[l].0, probe, 0))
                    .join()
                    .expect("probe panicked")
            };
            assert_eq!(
                probe_ticket,
                total as u64 + 1,
                "lock {l}: executions without a response",
            );
            tickets[l][probe].push(probe_ticket);
            let executions = &locks[l].1;
            for (worker, worker_tickets) in tickets[l].iter().enumerate() {
                assert_eq!(
                    executions[worker].load(Relaxed),
                    worker_tickets.len(),
                    "lock {l}, worker {worker}: executions != requests",
                );
            }
            let mut all: Vec<u64> = tickets[l].concat();
            all.sort_unstable();
            assert_eq!(
                all,
                (1..=total as u64 + 1).collect::<Vec<_>>(),
                "lock {l}: missing or duplicated execution",
            );
        }
    }

    macro_rules! per_queue {
        ($name:ident, $queue:ty) => {
            mod $name {
                use super::*;

                #[test]
                fn phased_stress() {
                    panic_after(Duration::from_secs(120), super::phased_stress::<$queue>);
                }

                #[test]
                fn enrollment_window() {
                    panic_after(Duration::from_secs(60), super::enrollment_window::<$queue>);
                }

                #[cfg(feature = "fcpq_fast_path_stat")]
                #[test]
                fn single_thread_hits() {
                    super::single_thread_hits::<$queue>();
                }

                #[test]
                #[serial_test::serial]
                fn thread_churn() {
                    panic_after(Duration::from_secs(120), super::thread_churn::<$queue>);
                }
            }
        };
    }

    per_queue!(btree, BTree);
    per_queue!(bheap, BHeap);
}
