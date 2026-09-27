//! Spin-then-park waiting for delegation locks (`spin_park` feature).
//!
//! A waiter first spins on `node.complete` (the pre-existing loop, retrying
//! `combiner_lock.try_lock()` every round). Once [`SPIN_BEFORE_PARK`] has
//! elapsed it parks on a per-node futex. Three Dekker-style handshakes keep
//! the design free of lost wakeups and of "parked with nobody combining":
//!
//! 1. Completion (waiter vs combiner). The waiter publishes `PARKED`
//!    (SeqCst), fences, then re-reads `complete`. The combiner publishes
//!    `complete = true` (Release), later fences, then reads the park word;
//!    if it is `PARKED` the combiner moves it to `NOTIFIED` with a CAS and
//!    calls `futex_wake`. By the SeqCst fence rule (C++ [atomics.fences]),
//!    whichever fence comes first in the total order, the other side's
//!    load observes the first side's store: either the waiter sees the
//!    result before sleeping, or the combiner sees `PARKED`. Because the
//!    wake always changes the futex word, a `futex_wait(PARKED)` that races
//!    with it returns immediately instead of sleeping. The combiner's fence
//!    may run any time after the result store; FC and FC-PQ run it after
//!    the *next* delegate (or at the end of the pass) so that the store
//!    buffer has drained and the fence does not stall on the result line
//!    the waiter is spinning on.
//!
//! 2. Progress (waiter vs unlocker). Before publishing `PARKED` the waiter
//!    increments the lock's `parked` counter; after the fence it makes one
//!    last `combiner_lock.try_lock()` and becomes the combiner if that
//!    succeeds. Every release of the combiner lock is `unlock; fence; load
//!    parked`, re-acquiring and combining while the counter is non-zero.
//!    Either the waiter sees the lock free, or the unlocker sees the count.
//!    A holder that wins `try_lock` in between inherits the obligation at
//!    its own release.
//!
//! 3. Enrollment (owner vs retirement). Handshake 2 only helps if the parked
//!    waiter's request is enrolled, so retirement of a node (FC cleanup,
//!    FC-PQ deactivation) is a CAS decision on `active` against the owner's
//!    SeqCst `complete = false`; exactly one side re-enrolls a request that
//!    raced with retirement. See `fc/lock.rs` and `fc_pq/lock.rs`.
//!
//! The counter, not a re-scan of the request list, is what the unlocker
//! checks: it is O(1), only written on park/unpark events, and it covers
//! re-requests by already-enrolled nodes that a "did the head change" test
//! would miss.

use std::{
    sync::atomic::{
        fence, AtomicBool, AtomicU32,
        Ordering::{AcqRel, Acquire, Relaxed, SeqCst},
    },
    time::{Duration, Instant},
};

use linux_futex::{Futex, Private};
use lock_api::RawMutex;

/// Spin budget per request before the waiter parks. Checked once per
/// 8-spin round; the first round arms the deadline, so an uncontended
/// request never reads the clock. The default (100 us) sits above the pass
/// length of the non-oversubscribed benchmark configurations (8-32 threads,
/// 1-3 us critical sections) so that waiters there keep spinning. Override
/// at build time with `DLOCK_SPIN_BEFORE_PARK_US=<decimal>` (cargo tracks
/// the variable through `option_env!`). Unit tests use a much shorter budget
/// so that the park paths are exercised.
pub const SPIN_BEFORE_PARK: Duration = Duration::from_micros(spin_before_park_us());

#[cfg(not(test))]
const DEFAULT_SPIN_BEFORE_PARK_US: u64 = 100;
#[cfg(test)]
const DEFAULT_SPIN_BEFORE_PARK_US: u64 = 5;

const fn spin_before_park_us() -> u64 {
    let Some(text) = option_env!("DLOCK_SPIN_BEFORE_PARK_US") else {
        return DEFAULT_SPIN_BEFORE_PARK_US;
    };
    let bytes = text.as_bytes();
    assert!(
        !bytes.is_empty(),
        "DLOCK_SPIN_BEFORE_PARK_US must be a decimal number of microseconds"
    );
    let mut value = 0u64;
    let mut i = 0;
    while i < bytes.len() {
        let digit = bytes[i];
        assert!(
            digit.is_ascii_digit(),
            "DLOCK_SPIN_BEFORE_PARK_US must be a decimal number of microseconds"
        );
        value = value * 10 + (digit - b'0') as u64;
        i += 1;
    }
    value
}

const EMPTY: u32 = 0;
const PARKED: u32 = 1;
const NOTIFIED: u32 = 2;

/// Per-node park word. The node's owner is the only writer of `EMPTY` and
/// `PARKED`; combiners only perform `PARKED -> NOTIFIED`.
#[derive(Debug, Default)]
pub struct ParkSlot {
    state: Futex<Private>,
}

/// Outcome of [`ParkSlot::park_or_lock`].
#[derive(Debug, PartialEq, Eq)]
pub enum Parked {
    /// `complete` is set; the owner may take its result.
    Complete,
    /// The last `try_lock` succeeded; the caller now holds the combiner lock.
    Combiner,
    /// Woken without a result (stale notification); re-enter the wait loop.
    Retry,
}

/// Waiter-side spin budget for one request.
#[derive(Debug, Default)]
pub struct SpinBudget {
    deadline: Option<Instant>,
}

impl SpinBudget {
    /// Called at the end of each spin round. Returns `true` once the request
    /// has spun for at least [`SPIN_BEFORE_PARK`].
    #[inline]
    pub fn exhausted(&mut self) -> bool {
        match self.deadline {
            None => {
                self.deadline = Some(Instant::now() + SPIN_BEFORE_PARK);
                false
            }
            Some(deadline) => Instant::now() >= deadline,
        }
    }
}

impl ParkSlot {
    pub const fn new() -> Self {
        Self {
            state: Futex::new(EMPTY),
        }
    }

    /// Waiter side. Preconditions: the calling thread owns the node, it has
    /// just enrolled or verified its enrollment (`active`), `complete` was
    /// last seen false and `combiner_lock.try_lock()` just failed. The word
    /// is `EMPTY` here because every exit below restores it and only the
    /// owner writes `PARKED`.
    ///
    /// Order: count, publish `PARKED`, fence, re-read `complete`, re-read
    /// `active` (a retirement that landed since the caller's check means the
    /// node must be re-enrolled: `Retry`), one last `try_lock`, then sleep.
    pub fn park_or_lock<L: RawMutex>(
        &self,
        parked: &AtomicU32,
        complete: &AtomicBool,
        active: &AtomicBool,
        combiner_lock: &L,
    ) -> Parked {
        // Count before the state is visible: a combiner that observes PARKED
        // (Acquire) therefore observes the increment its decrement balances.
        parked.fetch_add(1, SeqCst);
        let previous = self.state.value.swap(PARKED, SeqCst);
        debug_assert_eq!(previous, EMPTY, "park word not reset by owner");
        // Handshake 2: `parked` is published before the try_lock below
        // (the unlocker's side is a Release unlock followed by a fence).
        fence(SeqCst);

        // Handshake 1: SeqCst load after the SeqCst PARKED store.
        if complete.load(SeqCst) {
            self.unpark(parked);
            return Parked::Complete;
        }
        if !active.load(SeqCst) {
            self.unpark(parked);
            return Parked::Retry;
        }
        if combiner_lock.try_lock() {
            self.unpark(parked);
            return Parked::Combiner;
        }

        // Sleep until a combiner moves the word off PARKED. WrongValue
        // (EAGAIN) and Interrupted are handled by re-reading the word.
        while self.state.value.load(Acquire) == PARKED {
            let _ = self.state.wait(PARKED);
        }
        // NOTIFIED: the combiner already decremented `parked`.
        self.state.value.store(EMPTY, Relaxed);

        if complete.load(Acquire) {
            Parked::Complete
        } else {
            Parked::Retry
        }
    }

    /// Owner side, abandoning a published `PARKED` without sleeping. Exactly
    /// one of this CAS and the combiner's `PARKED -> NOTIFIED` CAS succeeds,
    /// and that side decrements `parked`.
    fn unpark(&self, parked: &AtomicU32) {
        match self
            .state
            .value
            .compare_exchange(PARKED, EMPTY, AcqRel, Acquire)
        {
            Ok(_) => {
                parked.fetch_sub(1, SeqCst);
            }
            Err(_) => {
                // A combiner notified us (and may issue a wasted futex wake).
                self.state.value.store(EMPTY, Relaxed);
            }
        }
    }

    /// Combiner side, any time after `complete.store(true, Release)` for
    /// this node (FC and FC-PQ defer it past the next delegate).
    #[inline]
    pub fn wake_if_parked(&self, parked: &AtomicU32) {
        // Handshake 1: the result store precedes this fence; the waiter's
        // PARKED store precedes its fence and its `complete` load.
        fence(SeqCst);
        if self.state.value.load(Relaxed) != PARKED {
            return;
        }
        if self
            .state
            .value
            .compare_exchange(PARKED, NOTIFIED, SeqCst, SeqCst)
            .is_ok()
        {
            parked.fetch_sub(1, SeqCst);
            self.state.wake(1);
        }
    }
}

/// Unlocker side, after `combiner_lock.unlock()`. Handshake 2: the release
/// is published before the counter is read. Returns `true` when some waiter
/// is parked and the caller must try to re-acquire and combine. The Acquire
/// load pairs with the parker's counter RMW, so the caller's next pass
/// observes everything that parker published before counting itself
/// (its `complete = false` and its list/ring enrollment).
#[inline]
pub fn parked_after_unlock(parked: &AtomicU32) -> bool {
    fence(SeqCst);
    parked.load(Acquire) != 0
}
