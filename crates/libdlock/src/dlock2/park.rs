//! Spin-then-park waiting for delegation locks: the `spin_park` and
//! `block_park` features (mutually exclusive). Both share the spin budget,
//! the lock-wide `parked` counter and the post-unlock check; they differ in
//! how a combiner wakes a parked waiter.
//!
//! # `spin_park`: wake after completion (deferred fence)
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
//!
//! # `block_park`: wake on pick (TCLocks `kombmtx.c`, OSDI'23 section 3.4)
//!
//! Same spin budget, same handshakes 2 and 3; handshake 1 is replaced by
//! two RMWs on the park word, one per side, so there is no fence, no
//! deferral and no wake after completion:
//!
//! - Owner, before publishing a request: `park = WAITING` (Relaxed; the
//!   SeqCst `complete = false` that publishes the request is sequenced
//!   after it, and every combiner-side RMW below follows an Acquire load of
//!   that `complete = false`, so the reset is first in the word's
//!   modification order for this request).
//! - Waiter, budget exhausted: `parked += 1`; `CAS(WAITING -> PARKED)`.
//!   If the CAS fails the word is `PICKED`: a combiner has taken the
//!   request and is running it or about to; undo the count and spin on
//!   `complete` (`Parked::Picked`). Otherwise fence, re-check `active` and
//!   `try_lock` (handshake 2), then `futex_wait(PARKED)`.
//! - Combiner, when it *picks* a request (FC: the scan reaches an active,
//!   incomplete node; FC-PQ: the entry is popped from the heap), before the
//!   delegate runs: `swap(PICKED)`; if the old value was `PARKED`, `parked
//!   -= 1` and `futex_wake`. Optional FC-PQ lookahead ([`WAKE_LOOKAHEAD`]
//!   = 1) applies the same swap to the next heap top so its wake latency
//!   overlaps the current delegate.
//!
//! Correctness of the wake: the waiter's CAS and the combiner's swap are
//! RMWs on the same word, so one is first in its modification order. If
//! the CAS is first, the swap reads `PARKED` and wakes; the wake changes
//! the futex word, so a `futex_wait(PARKED)` that has not slept yet returns
//! `EAGAIN`. If the swap is first, the CAS fails and the waiter never
//! sleeps. A woken or CAS-failed waiter spins on `complete` and never parks
//! again for this request: `PICKED` is terminal for the request, and the
//! combiner that set it either serves the request in the same pass (a
//! pick) or, for a lookahead wake at the pass limit, leaves it enrolled for
//! the next holder, which the spinning waiter's `try_lock` polling can
//! itself become. The result read stays behind the Acquire of
//! `complete = true`, exactly as without the feature.

#[cfg(all(feature = "spin_park", feature = "block_park"))]
compile_error!("`spin_park` and `block_park` are mutually exclusive park strategies");

/// Loom model of the `block_park` handshakes; see the module docs for the
/// command line.
#[cfg(all(loom, test, feature = "block_park"))]
mod loom_model;

#[cfg(feature = "spin_park")]
use std::sync::atomic::Ordering::AcqRel;
use std::{
    sync::atomic::{
        fence, AtomicBool, AtomicU32,
        Ordering::{Acquire, Relaxed, SeqCst},
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
    parse_decimal(
        text,
        "DLOCK_SPIN_BEFORE_PARK_US must be a decimal number of microseconds",
    )
}

/// FC-PQ wake lookahead under `block_park`: 0 wakes only the picked
/// request; 1 also wakes the next heap top when a request is picked.
/// Build-time override `DLOCK_WAKE_LOOKAHEAD=<0|1>`.
#[cfg(feature = "block_park")]
pub const WAKE_LOOKAHEAD: u64 = wake_lookahead();

#[cfg(feature = "block_park")]
const fn wake_lookahead() -> u64 {
    let Some(text) = option_env!("DLOCK_WAKE_LOOKAHEAD") else {
        return 0;
    };
    let value = parse_decimal(text, "DLOCK_WAKE_LOOKAHEAD must be 0 or 1");
    assert!(value <= 1, "DLOCK_WAKE_LOOKAHEAD must be 0 or 1");
    value
}

const fn parse_decimal(text: &str, message: &str) -> u64 {
    let bytes = text.as_bytes();
    assert!(!bytes.is_empty(), "{}", message);
    let mut value = 0u64;
    let mut i = 0;
    while i < bytes.len() {
        let digit = bytes[i];
        assert!(digit.is_ascii_digit(), "{}", message);
        value = value * 10 + (digit - b'0') as u64;
        i += 1;
    }
    value
}

#[cfg(feature = "spin_park")]
const EMPTY: u32 = 0;
#[cfg(feature = "spin_park")]
const PARKED: u32 = 1;
#[cfg(feature = "spin_park")]
const NOTIFIED: u32 = 2;

/// `block_park` states. The owner is the only writer of `WAITING` and the
/// only one that makes `WAITING -> PARKED` (CAS) or `PARKED -> WAITING`
/// (CAS, abandoning a park); combiners only `swap` in `PICKED`.
#[cfg(feature = "block_park")]
const WAITING: u32 = 0;
#[cfg(feature = "block_park")]
const PARKED: u32 = 1;
#[cfg(feature = "block_park")]
const PICKED: u32 = 2;

/// Per-node park word. `spin_park`: the node's owner is the only writer of
/// `EMPTY` and `PARKED`; combiners only perform `PARKED -> NOTIFIED`.
/// `block_park`: see the state constants.
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
    /// Woken without a result (stale notification) or found unenrolled;
    /// re-enter the wait loop.
    Retry,
    /// `block_park`: a combiner picked the request (before the park, or
    /// this wake). Spin on `complete`; do not park again for this request.
    #[cfg(feature = "block_park")]
    Picked,
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
            // `EMPTY` (`spin_park`) and `WAITING` (`block_park`) are both 0.
            state: Futex::new(0),
        }
    }
}

#[cfg(feature = "spin_park")]
impl ParkSlot {
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

#[cfg(feature = "block_park")]
impl ParkSlot {
    /// Owner side, before publishing a request (`complete = false`). Clears
    /// the `PICKED` left by the previous request; the request's own
    /// combiner-side swaps all follow its publication, so none precedes
    /// this store in the word's modification order.
    #[inline]
    pub fn reset(&self) {
        self.state.value.store(WAITING, Relaxed);
    }

    /// Waiter side. Preconditions: the calling thread owns the node, it has
    /// just enrolled or verified its enrollment (`active`), `complete` was
    /// last seen false, `combiner_lock.try_lock()` just failed, and no
    /// earlier call for this request returned `Picked`.
    ///
    /// Order: count, `CAS(WAITING -> PARKED)` (failure: `Picked`), fence,
    /// re-read `complete` and `active` (a retirement that landed since the
    /// caller's check means the node must be re-enrolled: `Retry`), one
    /// last `try_lock`, then sleep until a combiner swaps in `PICKED`.
    pub fn park_or_lock<L: RawMutex>(
        &self,
        parked: &AtomicU32,
        complete: &AtomicBool,
        active: &AtomicBool,
        combiner_lock: &L,
    ) -> Parked {
        // Count before the state is visible: a combiner whose swap reads
        // PARKED is later in the SeqCst order than this increment, so its
        // decrement balances it (the counter never wraps below zero).
        parked.fetch_add(1, SeqCst);
        if self
            .state
            .value
            .compare_exchange(WAITING, PARKED, SeqCst, SeqCst)
            .is_err()
        {
            // PICKED: the combiner has the request; it did not see PARKED,
            // so this side owns the undo.
            parked.fetch_sub(1, SeqCst);
            return Parked::Picked;
        }
        // Handshake 2: `parked` is published before the try_lock below
        // (the unlocker's side is a Release unlock followed by a fence).
        fence(SeqCst);

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

        // Sleep until a combiner swaps in PICKED. WrongValue (EAGAIN) and
        // Interrupted are handled by re-reading the word.
        while self.state.value.load(Acquire) == PARKED {
            let _ = self.state.wait(PARKED);
        }
        // PICKED: the combiner that read PARKED already decremented
        // `parked` and is running (or about to run) the request.
        if complete.load(Acquire) {
            Parked::Complete
        } else {
            Parked::Picked
        }
    }

    /// Owner side, abandoning a published `PARKED` without sleeping. Exactly
    /// one of this CAS and a combiner's swap reads `PARKED`, and that side
    /// decrements `parked`. On failure the word is `PICKED` and stays so:
    /// the caller's next park attempt returns `Picked` without sleeping.
    fn unpark(&self, parked: &AtomicU32) {
        if self
            .state
            .value
            .compare_exchange(PARKED, WAITING, SeqCst, SeqCst)
            .is_ok()
        {
            parked.fetch_sub(1, SeqCst);
        }
    }

    /// Combiner side, when it picks this node's request, before the delegate
    /// runs (FC-PQ lookahead: also for the next heap top). Wakes the owner
    /// iff it was parked; the swap itself makes a not-yet-sleeping
    /// `futex_wait(PARKED)` return.
    #[inline]
    pub fn pick(&self, parked: &AtomicU32) {
        if self.state.value.swap(PICKED, SeqCst) == PARKED {
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
