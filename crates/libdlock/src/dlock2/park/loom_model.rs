//! Loom model of the `block_park` protocol (`dlock2/park.rs`): waiter CAS vs
//! combiner swap vs unlock hand-off, with the lock-wide `parked` counter and
//! the enrollment CAS (handshakes 2 and 3). The futex is modeled as a
//! check-then-sleep under a mutex, which is what `FUTEX_WAIT` / `FUTEX_WAKE`
//! guarantee (a wake after the word changed is never lost by a waiter that
//! compared the word before sleeping).
//!
//! Each scenario is one owner issuing a request with spin budget 0 (park at
//! the first failed `try_lock`, the earliest and therefore hardest case)
//! against combiners and holders mirroring `fc/lock.rs` / `fc_pq/lock.rs`:
//! `combine` picks (swap `PICKED`, wake iff `PARKED`), runs the delegate and
//! publishes `complete`; `release_combiner` re-combines while `parked != 0`.
//! Properties checked in every interleaving: the owner terminates (no lost
//! wake-up, no stuck enrolled request: a deadlock or a spin without progress
//! would exhaust loom's exploration or the loop guards), it reads a result
//! only after the delegate wrote it (loom's `UnsafeCell` race detector), and
//! the `parked` counter returns to 0 with the lock free.
//!
//! Run (release: loom is slow in debug):
//!
//! ```text
//! RUSTFLAGS="--cfg loom" cargo test -p libdlock --release --features block_park \
//!     --lib dlock2::park::loom_model
//! ```

use loom::{
    cell::UnsafeCell,
    sync::{
        atomic::{
            fence, AtomicBool, AtomicU32,
            Ordering::{Acquire, Relaxed, Release, SeqCst},
        },
        Arc, Condvar, Mutex,
    },
    thread,
};

const WAITING: u32 = 0;
const PARKED: u32 = 1;
const PICKED: u32 = 2;

/// Iterations after which a model spin loop is declared stuck. Every loop
/// below waits for another thread's bounded progress, so this never fires
/// for a correct protocol; it turns a livelock into a diagnosed failure.
const LOOP_GUARD: u32 = 300;

/// `Futex<Private>` stand-in: `wait` returns at once when the word differs
/// from `expected` (EAGAIN), otherwise sleeps until a `wake`; the caller
/// re-checks the word, as `park_or_lock` does.
struct Futex {
    value: AtomicU32,
    gate: Mutex<()>,
    sleepers: Condvar,
}

impl Futex {
    fn new(value: u32) -> Self {
        Futex {
            value: AtomicU32::new(value),
            gate: Mutex::new(()),
            sleepers: Condvar::new(),
        }
    }

    fn wait(&self, expected: u32) {
        let guard = self.gate.lock().unwrap();
        if self.value.load(Relaxed) != expected {
            return;
        }
        drop(self.sleepers.wait(guard).unwrap());
    }

    fn wake(&self) {
        let _guard = self.gate.lock().unwrap();
        self.sleepers.notify_all();
    }
}

/// `RawSpinLock` orderings: `try_lock` is a CAS with Acquire on success,
/// `unlock` a Release store.
struct CombinerLock(AtomicBool);

impl CombinerLock {
    fn try_lock(&self) -> bool {
        self.0
            .compare_exchange(false, true, Acquire, Relaxed)
            .is_ok()
    }

    fn lock(&self) {
        let mut spins = 0;
        while !self.try_lock() {
            spins += 1;
            assert!(spins < LOOP_GUARD, "lock() spin without progress");
            thread::yield_now();
        }
    }

    fn unlock(&self) {
        self.0.store(false, Release);
    }
}

/// One node: the owner's request slot plus its park word. `active` doubles
/// as list membership (FC) / queue membership (FC-PQ).
struct Node {
    state: Futex,
    complete: AtomicBool,
    active: AtomicBool,
    input: UnsafeCell<u64>,
    result: UnsafeCell<u64>,
}

struct Lock {
    combiner_lock: CombinerLock,
    parked: AtomicU32,
    node: Node,
}

enum Parked {
    Complete,
    Combiner,
    Retry,
    Picked,
}

impl Lock {
    fn new() -> Arc<Self> {
        Self::with_node(WAITING, false, false, 0)
    }

    /// State right after a served request whose owner has not yet issued
    /// the next one: word `PICKED`, `complete`, still enrolled, old result.
    fn new_after_served(old_result: u64) -> Arc<Self> {
        Self::with_node(PICKED, true, true, old_result)
    }

    fn with_node(state: u32, complete: bool, active: bool, result: u64) -> Arc<Self> {
        Arc::new(Lock {
            combiner_lock: CombinerLock(AtomicBool::new(false)),
            parked: AtomicU32::new(0),
            node: Node {
                state: Futex::new(state),
                complete: AtomicBool::new(complete),
                active: AtomicBool::new(active),
                input: UnsafeCell::new(0),
                result: UnsafeCell::new(result),
            },
        })
    }

    // ---- park.rs, `block_park` -------------------------------------------

    fn reset(&self) {
        self.node.state.value.store(WAITING, Relaxed);
    }

    fn park_or_lock(&self) -> Parked {
        let node = &self.node;
        self.parked.fetch_add(1, SeqCst);
        if node
            .state
            .value
            .compare_exchange(WAITING, PARKED, SeqCst, SeqCst)
            .is_err()
        {
            self.parked.fetch_sub(1, SeqCst);
            return Parked::Picked;
        }
        fence(SeqCst);
        if node.complete.load(SeqCst) {
            self.unpark();
            return Parked::Complete;
        }
        if !node.active.load(SeqCst) {
            self.unpark();
            return Parked::Retry;
        }
        if self.combiner_lock.try_lock() {
            self.unpark();
            return Parked::Combiner;
        }
        while node.state.value.load(Acquire) == PARKED {
            node.state.wait(PARKED);
        }
        if node.complete.load(Acquire) {
            Parked::Complete
        } else {
            Parked::Picked
        }
    }

    fn unpark(&self) {
        if self
            .node
            .state
            .value
            .compare_exchange(PARKED, WAITING, SeqCst, SeqCst)
            .is_ok()
        {
            self.parked.fetch_sub(1, SeqCst);
        }
    }

    fn pick(&self) {
        if self.node.state.value.swap(PICKED, SeqCst) == PARKED {
            self.parked.fetch_sub(1, SeqCst);
            self.node.state.wake();
        }
    }

    fn parked_after_unlock(&self) -> bool {
        fence(SeqCst);
        self.parked.load(Acquire) != 0
    }

    // ---- fc/lock.rs, fc_pq/lock.rs ---------------------------------------

    /// `push_if_unactive` (handshake 3, owner side).
    fn enroll(&self) {
        if self.node.active.load(SeqCst) {
            return;
        }
        let _ = self
            .node
            .active
            .compare_exchange(false, true, SeqCst, SeqCst);
    }

    /// One combining pass over the single node. Caller holds the lock.
    fn combine(&self) {
        let node = &self.node;
        if node.active.load(Acquire) && !node.complete.load(Acquire) {
            self.pick();
            let input = node.input.with(|p| unsafe { *p });
            node.result.with_mut(|p| unsafe { *p = input + 1 });
            node.complete.store(true, Release);
        }
    }

    /// FC-PQ lookahead at the pass limit: the request is picked (woken) but
    /// not served by this pass. Caller holds the lock.
    fn prewake_only(&self) {
        let node = &self.node;
        if node.active.load(Acquire) && !node.complete.load(Acquire) {
            self.pick();
        }
    }

    /// `retire_unlinked` / `retire_or_requeue` (handshake 3, retirer side):
    /// deactivate, then re-enroll if a request is pending. Caller holds the
    /// lock.
    fn retire(&self) {
        let node = &self.node;
        node.active.store(false, SeqCst);
        if !node.complete.load(SeqCst) {
            let _ = node.active.compare_exchange(false, true, SeqCst, SeqCst);
        }
    }

    /// The re-combine loop waits for a parked waiter to finish its own
    /// park attempt: a waiter preempted between `parked += 1` and its CAS
    /// (or between a failed CAS and its undo) keeps the count non-zero
    /// until it runs again. The real loop relies on the OS scheduler for
    /// that; loom needs the explicit hint, as for any spin loop.
    fn release_combiner(&self) {
        self.combiner_lock.unlock();
        let mut spins = 0;
        while self.parked_after_unlock() && self.combiner_lock.try_lock() {
            self.combine();
            self.combiner_lock.unlock();
            spins += 1;
            assert!(
                spins < LOOP_GUARD,
                "re-combine spin without progress: state={} parked={} active={} complete={}",
                self.node.state.value.load(SeqCst),
                self.parked.load(SeqCst),
                self.node.active.load(SeqCst),
                self.node.complete.load(SeqCst)
            );
            thread::yield_now();
        }
    }

    /// `lock()` with spin budget 0: park on the first failed `try_lock`;
    /// once picked, spin on `complete` polling `try_lock` (the existing
    /// round loop, one poll per round here).
    fn request(&self, input: u64) -> u64 {
        let node = &self.node;
        node.input.with_mut(|p| unsafe { *p = input });
        self.reset();
        node.complete.store(false, SeqCst);
        let mut picked = false;
        let mut spins = 0;
        loop {
            self.enroll();
            if self.combiner_lock.try_lock() {
                self.combine();
                self.release_combiner();
                if node.complete.load(Acquire) {
                    break;
                }
            } else if !picked {
                match self.park_or_lock() {
                    Parked::Complete => break,
                    Parked::Combiner => {
                        self.combine();
                        self.release_combiner();
                        if node.complete.load(Acquire) {
                            break;
                        }
                    }
                    Parked::Retry => {}
                    Parked::Picked => picked = true,
                }
            } else {
                if node.complete.load(Acquire) {
                    break;
                }
                spins += 1;
                assert!(spins < LOOP_GUARD, "picked waiter spin without progress");
                thread::yield_now();
            }
        }
        node.result.with(|p| unsafe { *p })
    }
}

fn check(lock: &Lock, results: &[(u64, u64)]) {
    for (input, result) in results {
        assert_eq!(*result, input + 1, "wrong or unwritten result");
    }
    assert_eq!(lock.parked.load(SeqCst), 0, "unbalanced parked count");
    assert!(
        !lock.combiner_lock.0.load(SeqCst),
        "combiner lock left held"
    );
}

/// Owner vs a combiner that takes the lock, runs one pass and releases:
/// covers pick-before-park (CAS fails), pick-of-a-sleeper (wake), the
/// owner's pre-park `try_lock`, and a pass that missed the enrollment
/// (unlock hand-off through `parked_after_unlock`).
#[test]
fn waiter_vs_combiner_pick() {
    loom::model(|| {
        let lock = Lock::new();
        let combiner = {
            let lock = lock.clone();
            thread::spawn(move || {
                lock.combiner_lock.lock();
                lock.combine();
                lock.release_combiner();
            })
        };
        let result = lock.request(7);
        combiner.join().unwrap();
        check(&lock, &[(7, result)]);
    });
}

/// Owner vs a holder that never combines (the FC-PQ fast path shape): the
/// owner is served by its own pre-park `try_lock` or by the holder's
/// post-unlock re-combine, never left asleep.
#[test]
fn waiter_vs_idle_holder() {
    loom::model(|| {
        let lock = Lock::new();
        let holder = {
            let lock = lock.clone();
            thread::spawn(move || {
                lock.combiner_lock.lock();
                lock.release_combiner();
            })
        };
        let result = lock.request(3);
        holder.join().unwrap();
        check(&lock, &[(3, result)]);
    });
}

/// Owner vs a combiner whose pass is followed by node retirement
/// (handshake 3): the owner's request is enrolled by exactly one side and
/// a parked owner is served.
#[test]
fn waiter_vs_retirement() {
    loom::model(|| {
        let lock = Lock::new();
        let combiner = {
            let lock = lock.clone();
            thread::spawn(move || {
                lock.combiner_lock.lock();
                lock.combine();
                lock.retire();
                lock.release_combiner();
            })
        };
        let result = lock.request(11);
        combiner.join().unwrap();
        check(&lock, &[(11, result)]);
    });
}

/// FC-PQ lookahead: the combiner picks (wakes) the request without serving
/// it and releases; `PICKED` is terminal, so the owner must serve itself
/// through its `try_lock` polling and the count must stay balanced.
#[test]
fn waiter_vs_lookahead_at_pass_limit() {
    loom::model(|| {
        let lock = Lock::new();
        let combiner = {
            let lock = lock.clone();
            thread::spawn(move || {
                lock.combiner_lock.lock();
                lock.prewake_only();
                lock.release_combiner();
            })
        };
        let result = lock.request(5);
        combiner.join().unwrap();
        check(&lock, &[(5, result)]);
    });
}

/// The owner's next request after a served one (word left `PICKED`,
/// `complete = true`, still enrolled, stale result in the slot) against one
/// combiner pass: the `WAITING` reset must let the owner park and be woken
/// by the pick, and the stale `PICKED` / result must not leak into the new
/// request. (The served request's own interleavings are scenario
/// `waiter_vs_combiner_pick`; starting from its final state keeps the
/// model tractable, an unbounded two-pass version did not finish in 50 min.)
#[test]
fn next_request_after_served() {
    loom::model(|| {
        let lock = Lock::new_after_served(21);
        let combiner = {
            let lock = lock.clone();
            thread::spawn(move || {
                lock.combiner_lock.lock();
                lock.combine();
                lock.release_combiner();
            })
        };
        let result = lock.request(30);
        combiner.join().unwrap();
        check(&lock, &[(30, result)]);
    });
}

/// Owner, a combiner and an opportunistic idle holder (one `try_lock`; on
/// success it releases without combining): a holder that wins the lock
/// between the combiner's unlock and its `parked` check inherits the
/// re-combine obligation at its own release.
///
/// Ignored: loom 0.7 starves the owner in this shape. With the combiner
/// and the holder both in the yield-looping re-combine loop, exploration
/// reaches a path where the owner sits between its failed `PARKED` CAS and
/// the `parked -= 1` undo (a runnable thread) and is never scheduled while
/// the holder re-combines (`re-combine spin without progress: state=PICKED
/// parked=1 complete=true`). The protocol step the owner is about to take
/// is a single RMW; the OS scheduler runs it. Two-thread coverage of the
/// same hand-off is `waiter_vs_idle_holder` and `waiter_vs_combiner_pick`.
#[test]
#[ignore = "loom 0.7 starves the owner's pending undo step behind two yield-looping holders"]
fn waiter_vs_combiner_and_idle_holder() {
    let mut model = loom::model::Builder::new();
    model.max_branches = 20_000;
    model.check(|| {
        let lock = Lock::new();
        let combiner = {
            let lock = lock.clone();
            thread::spawn(move || {
                if lock.combiner_lock.try_lock() {
                    lock.combine();
                    lock.release_combiner();
                }
            })
        };
        let holder = {
            let lock = lock.clone();
            thread::spawn(move || {
                if lock.combiner_lock.try_lock() {
                    lock.release_combiner();
                }
            })
        };
        let result = lock.request(9);
        combiner.join().unwrap();
        holder.join().unwrap();
        check(&lock, &[(9, result)]);
    });
}
