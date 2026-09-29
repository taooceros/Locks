//! `dispatch-pq`: an async mutex with usage-ordered handoff and no
//! delegation (u-SCL style). Kill test for "cheap service fairness needs a
//! combiner".
//!
//! As in [`super::dispatch`], the critical section runs on the client's own
//! task and worker: acquire, run the closure with `cycles()` around it
//! (charged to this client's usage), release. What differs is the successor:
//!
//! - waiters sit in FC-PQ's [`UsageQueue`] (reused, not copied): min
//!   cumulative charged cycles first, FIFO among equals; the newcomer init
//!   (a never-served client enters at the lock's running mean cost per
//!   request by default) and the starvation clamp are FC-PQ's, taken from
//!   [`DispatchPqOptions::queue`];
//! - the clamp's clock is the **handoff** instead of the combining pass: an
//!   entry that has seen more than `starvation_clamp` handoffs go to other
//!   waiters has its key lowered to the heap minimum at every later handoff
//!   until it is served (default [`DEFAULT_STARVATION_CLAMP`] = 16; 0 = off);
//!   `record_waits` counts waits in handoffs;
//! - release with waiters hands ownership directly to the heap minimum (the
//!   lock stays held) and wakes it with [`DispatchPqOptions::wake_placement`]
//!   (`Default` = `dispatch`'s tokio-style local-queue wake); without waiters
//!   it unlocks;
//! - an uncontended acquire is one CAS on the lock word; the queue is touched
//!   only while the lock is held.
//!
//! Accounting difference to FC-PQ: a request that takes the uncontended
//! fast path (or finds the lock free under the queue spinlock) is charged on
//! top of the client's own usage and never gets the newcomer init. FC-PQ
//! routes every request through the heap.
//!
//! ## Lock word and memory model
//!
//! `state` is `UNLOCKED`, `LOCKED` or `LOCKED | QUEUED`. `QUEUED` implies
//! `LOCKED`, and under the queue spinlock `QUEUED` is set iff the queue is
//! non-empty. While `QUEUED` is set only spinlock holders write `state`: the
//! fast acquire CAS needs `UNLOCKED`, the fast release CAS needs exactly
//! `LOCKED`.
//!
//! - Fast acquire: CAS `UNLOCKED -> LOCKED` (Acquire), pairing with the
//!   Release of whichever release left the lock free.
//! - Fast release: CAS `LOCKED -> UNLOCKED` (Release); it fails iff an
//!   enqueuer set `QUEUED`.
//! - Enqueue, under the spinlock: CAS loop on `state`; if it is free, take it
//!   (the acquisition "found free"); else set `QUEUED` and push. The `QUEUED`
//!   CAS is AcqRel and the releaser's failing fast CAS is Acquire, so the
//!   enqueuer's spinlock acquisition happens-before the releaser's; the
//!   releaser therefore takes the spinlock only after the push. That is the
//!   no-lost-wakeup argument for the release/enqueue race.
//! - Slow release, under the spinlock: pop the minimum, clear `QUEUED` if the
//!   queue is now empty (ownership moves, `LOCKED` stays), write the grant
//!   (accounting base, instrumentation), `granted.store(true, Release)`,
//!   release the spinlock, then wake. The waiter's `granted.load(Acquire)`
//!   orders the previous critical section before its own.
//! - Mean newcomer init reads the lock's running totals, which each owner
//!   updates after its critical section (exclusive by protocol; Relaxed
//!   atomics so that enqueuers may read a slightly stale snapshot).
//!
//! ## Node lifetime
//!
//! Each client owns one heap-allocated [`Waiter`] (allocated in
//! [`DelegationLock::client`], freed when the client is dropped; no
//! per-request allocation). The queue holds raw pointers only to waiters
//! whose client is mutably borrowed by a pending `run` future. Dropping that
//! future while queued unlinks the waiter, or, if ownership was already
//! granted, passes the lock on; leaking it with `mem::forget` is excluded
//! (contract in `lock.rs`). A closure that panics leaves the lock held.

use std::cell::UnsafeCell;
use std::future::Future;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use crossbeam_utils::CachePadded;
use parking_lot::Mutex;

use super::fc_pq::{FcPqOptions, UsageNode, UsageQueue};
use crate::executor::{self, Placement};
use crate::lock::{cycles, DelegationLock, LockClient};
use crate::stats;

/// Default starvation clamp, in handoffs (numerically the clamp of
/// `fcpq-h16-home-c16`, whose unit is a combining pass).
pub const DEFAULT_STARVATION_CLAMP: u64 = 16;

const UNLOCKED: u8 = 0;
const LOCKED: u8 = 1;
const QUEUED: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DispatchPqOptions {
    /// Placement of the handoff wake. `Default` wakes like `dispatch`
    /// (local queue of the releasing worker); `Inline` is treated as
    /// `Default`.
    pub wake_placement: Placement,
    /// Queue policy: `starvation_clamp` (in handoffs), `newcomer_init`,
    /// `record_waits` (waits in handoffs, via `fc_pq::take_wait_stats`).
    /// The combining knobs (`pass_limit`, mitigations, yield) are unused.
    pub queue: FcPqOptions,
    /// Record [`HandoffStats`] while `stats::recording()` is on (a few extra
    /// `cycles()` reads on the release path).
    pub record_handoffs: bool,
}

impl Default for DispatchPqOptions {
    fn default() -> Self {
        Self {
            wake_placement: Placement::Default,
            queue: FcPqOptions {
                starvation_clamp: DEFAULT_STARVATION_CLAMP,
                ..FcPqOptions::default()
            },
            record_handoffs: false,
        }
    }
}

/// Where acquisitions came from and where a handoff's cycles go (TSC
/// cycles, summed). A handoff is timed from the releaser's critical-section
/// end to the grantee's critical-section start:
/// `release_spin + release_queue + grant_to_start`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HandoffStats {
    /// Acquisitions by the uncontended CAS (queue untouched).
    pub fast: u64,
    /// Acquisitions that failed the CAS but found the lock free under the
    /// queue spinlock (the owner released in between).
    pub free: u64,
    /// Acquisitions by handoff from a releasing owner (only grants issued
    /// while recording; a grant spanning the start of recording is skipped).
    pub handoffs: u64,
    /// Releaser: critical-section end to queue spinlock acquired.
    pub release_spin_cycles: u64,
    /// Releaser: spinlock acquired to grant published (clamp scan and
    /// rekey, heap pop, waker take).
    pub release_queue_cycles: u64,
    /// Grant published to the grantee's critical-section start (spinlock
    /// release, wake call, run-queue wait, poll, grant check).
    pub grant_to_start_cycles: u64,
    /// Requests that queued, and their cycles from the failed fast CAS to
    /// the spinlock release after the push (off the lock's critical path
    /// except through spinlock contention).
    pub enqueues: u64,
    pub enqueue_cycles: u64,
}

impl HandoffStats {
    fn add(&mut self, o: &HandoffStats) {
        self.fast += o.fast;
        self.free += o.free;
        self.handoffs += o.handoffs;
        self.release_spin_cycles += o.release_spin_cycles;
        self.release_queue_cycles += o.release_queue_cycles;
        self.grant_to_start_cycles += o.grant_to_start_cycles;
        self.enqueues += o.enqueues;
        self.enqueue_cycles += o.enqueue_cycles;
    }
}

/// Stats of the last dropped lock that recorded anything.
static LAST_HANDOFF_STATS: Mutex<Option<HandoffStats>> = Mutex::new(None);

/// Take the [`HandoffStats`] of the most recently dropped [`DispatchPq`]
/// that recorded any (the harness drops the lock when `run_raw` returns).
pub fn take_handoff_stats() -> Option<HandoffStats> {
    LAST_HANDOFF_STATS.lock().take()
}

// ---------------------------------------------------------------------------
// Waiter
// ---------------------------------------------------------------------------

/// Per-client node: queue entry target plus the client's accounting.
/// `repr(C)` keeps the fields a releaser writes on the first cache line.
#[repr(C, align(128))]
pub struct Waiter {
    /// Guarded by the queue spinlock.
    waker: UnsafeCell<Option<Waker>>,
    /// Release by the granting releaser (under the spinlock), Acquire by the
    /// owner.
    granted: AtomicBool,
    /// Grant payload, written by the releaser before the `granted` Release
    /// store: the request's accounting base (usage at admission, or the
    /// newcomer init).
    grant_base: AtomicU64,
    /// `record_handoffs` only: grant timestamp (0 = grant issued while not
    /// recording, i.e. untimed), releaser spin and queue cycles of this
    /// grant.
    grant_tsc: AtomicU64,
    grant_spin: AtomicU64,
    grant_queue: AtomicU64,
    /// Cumulative charged cycles and requests served. Written only by the
    /// owning client (after its own critical section), read by it at
    /// admission.
    usage: AtomicU64,
    served: AtomicU64,
    /// Owner-only instrumentation, folded into the lock on client drop.
    stats: UnsafeCell<HandoffStats>,
}

// SAFETY: `waker` is touched only under the queue spinlock; `stats` only by
// the owning client; everything else is atomic.
unsafe impl Send for Waiter {}
unsafe impl Sync for Waiter {}

impl Waiter {
    fn new() -> Self {
        Waiter {
            waker: UnsafeCell::new(None),
            granted: AtomicBool::new(false),
            grant_base: AtomicU64::new(0),
            grant_tsc: AtomicU64::new(0),
            grant_spin: AtomicU64::new(0),
            grant_queue: AtomicU64::new(0),
            usage: AtomicU64::new(0),
            served: AtomicU64::new(0),
            stats: UnsafeCell::new(HandoffStats::default()),
        }
    }

    /// Owner only, after its release (the node is unlinked, so the grant
    /// fields are stable until it queues again).
    fn record(&self, how: Acquired, cs_start: u64) {
        // SAFETY: owner-only field.
        let s = unsafe { &mut *self.stats.get() };
        match how {
            Acquired::Fast => s.fast += 1,
            Acquired::Free => s.free += 1,
            Acquired::Handoff => {
                let granted_at = self.grant_tsc.load(Relaxed);
                if granted_at == 0 {
                    // Granted before recording started: no timing to count.
                    return;
                }
                s.handoffs += 1;
                s.release_spin_cycles += self.grant_spin.load(Relaxed);
                s.release_queue_cycles += self.grant_queue.load(Relaxed);
                s.grant_to_start_cycles += cs_start.saturating_sub(granted_at);
            }
        }
    }

    /// Owner only.
    fn record_enqueue(&self, cycles: u64) {
        // SAFETY: owner-only field.
        let s = unsafe { &mut *self.stats.get() };
        s.enqueues += 1;
        s.enqueue_cycles += cycles;
    }
}

impl UsageNode for Waiter {
    fn usage(&self) -> u64 {
        self.usage.load(Relaxed)
    }
    fn served(&self) -> u64 {
        self.served.load(Relaxed)
    }
    fn charge(&self, usage: u64) {
        self.usage.store(usage, Relaxed);
        self.served.store(self.served.load(Relaxed) + 1, Relaxed);
    }
    fn set_usage(&self, usage: u64) {
        self.usage.store(usage, Relaxed);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Acquired {
    Fast,
    Free,
    Handoff,
}

// ---------------------------------------------------------------------------
// Lock
// ---------------------------------------------------------------------------

/// Lock word, queue spinlock and the Mean-newcomer totals: every acquire and
/// release touches this line.
struct Hot {
    state: AtomicU8,
    spin: AtomicBool,
    /// Sum of charged critical-section cycles and requests served, over the
    /// lock's lifetime. Owner-written, enqueuer-read.
    usage_total: AtomicU64,
    served_total: AtomicU64,
}

/// Spinlock-guarded.
struct Queue {
    pq: UsageQueue<Waiter>,
    /// Handoffs so far: the starvation clamp's clock.
    handoffs: u64,
}

pub struct DispatchPq<T> {
    hot: CachePadded<Hot>,
    queue: CachePadded<UnsafeCell<Queue>>,
    data: UnsafeCell<T>,
    opts: DispatchPqOptions,
    /// `record_handoffs`: stats of dropped clients.
    stats: Mutex<HandoffStats>,
}

// SAFETY: `queue` is accessed only under `hot.spin`; `data` only by the lock
// owner; raw waiter pointers refer to waiters of borrowed clients.
unsafe impl<T: Send> Send for DispatchPq<T> {}
unsafe impl<T: Send> Sync for DispatchPq<T> {}

impl<T> Drop for DispatchPq<T> {
    fn drop(&mut self) {
        let s = *self.stats.get_mut();
        if s != HandoffStats::default() {
            *LAST_HANDOFF_STATS.lock() = Some(s);
        }
    }
}

impl<T: Send + 'static> DispatchPq<T> {
    pub fn with_options(data: T, opts: DispatchPqOptions) -> Self {
        let opts = DispatchPqOptions {
            wake_placement: match opts.wake_placement {
                Placement::Inline => Placement::Default,
                p => p,
            },
            ..opts
        };
        DispatchPq {
            hot: CachePadded::new(Hot {
                state: AtomicU8::new(UNLOCKED),
                spin: AtomicBool::new(false),
                usage_total: AtomicU64::new(0),
                served_total: AtomicU64::new(0),
            }),
            queue: CachePadded::new(UnsafeCell::new(Queue {
                pq: UsageQueue::new(opts.queue),
                handoffs: 0,
            })),
            data: UnsafeCell::new(data),
            opts,
            stats: Mutex::new(HandoffStats::default()),
        }
    }

    pub fn options(&self) -> DispatchPqOptions {
        self.opts
    }
}

impl<T> DispatchPq<T> {
    #[inline]
    fn recording(&self) -> bool {
        self.opts.record_handoffs && stats::recording()
    }

    #[inline]
    fn lock_spin(&self) {
        let spin = &self.hot.spin;
        loop {
            if spin
                .compare_exchange_weak(false, true, Acquire, Relaxed)
                .is_ok()
            {
                return;
            }
            while spin.load(Relaxed) {
                std::hint::spin_loop();
            }
        }
    }

    #[inline]
    fn unlock_spin(&self) {
        self.hot.spin.store(false, Release);
    }

    #[inline]
    fn try_lock_fast(&self) -> bool {
        self.hot
            .state
            .compare_exchange(UNLOCKED, LOCKED, Acquire, Relaxed)
            .is_ok()
    }

    /// Slow acquire. Returns `true` if the lock was free (now ours);
    /// otherwise `node` is queued with `cx`'s waker.
    fn enqueue_or_acquire(&self, node: &Waiter, cx: &Context<'_>) -> bool {
        node.granted.store(false, Relaxed);
        self.lock_spin();
        let state = &self.hot.state;
        let mut s = state.load(Relaxed);
        loop {
            let want = if s == UNLOCKED {
                LOCKED
            } else if s & QUEUED != 0 {
                break;
            } else {
                s | QUEUED
            };
            match state.compare_exchange_weak(s, want, AcqRel, Relaxed) {
                Ok(_) if want == LOCKED => {
                    self.unlock_spin();
                    return true;
                }
                Ok(_) => break,
                Err(actual) => s = actual,
            }
        }
        // SAFETY: spinlock held; the node is not linked yet. The waker clone
        // is a reference-count increment (no allocation).
        unsafe { *node.waker.get() = Some(cx.waker().clone()) };
        // SAFETY: spinlock held.
        let q = unsafe { &mut *self.queue.get() };
        if node.served() == 0 {
            q.pq.set_totals(
                self.hot.usage_total.load(Relaxed),
                self.hot.served_total.load(Relaxed),
            );
        }
        q.pq.push(NonNull::from(node), q.handoffs + 1);
        self.unlock_spin();
        false
    }

    /// Re-poll of a queued waiter: `true` once ownership was granted,
    /// otherwise refreshes the stored waker.
    fn poll_granted(&self, node: &Waiter, cx: &Context<'_>) -> bool {
        if node.granted.load(Acquire) {
            return true;
        }
        self.lock_spin();
        if node.granted.load(Acquire) {
            self.unlock_spin();
            return true;
        }
        // SAFETY: spinlock held, node linked and not granted.
        unsafe {
            let w = &mut *node.waker.get();
            if !w.as_ref().is_some_and(|w| w.will_wake(cx.waker())) {
                *w = Some(cx.waker().clone());
            }
        }
        self.unlock_spin();
        false
    }

    /// Owner only: fold a finished request's charge into the running totals.
    #[inline]
    fn account(&self, cs: u64) {
        let h = &*self.hot;
        h.usage_total
            .store(h.usage_total.load(Relaxed).wrapping_add(cs), Relaxed);
        h.served_total
            .store(h.served_total.load(Relaxed) + 1, Relaxed);
    }

    /// Owner only: release after a critical section that ended at `t_end`.
    /// With waiters, ownership passes to the minimum-usage one and its waker
    /// is returned for the caller to wake.
    fn release(&self, t_end: u64) -> Option<Waker> {
        let state = &self.hot.state;
        if state
            .compare_exchange(LOCKED, UNLOCKED, Release, Acquire)
            .is_ok()
        {
            return None;
        }
        self.lock_spin();
        let rec = self.recording();
        let t_locked = if rec { cycles() } else { 0 };
        // SAFETY: spinlock held.
        let q = unsafe { &mut *self.queue.get() };
        if q.pq.is_empty() {
            // Every waiter cancelled after our CAS saw QUEUED; `cancel`
            // already cleared the bit.
            state.store(UNLOCKED, Release);
            self.unlock_spin();
            return None;
        }
        q.handoffs += 1;
        q.pq.promote_starving(q.handoffs);
        let (node_ptr, base) = q.pq.pop().expect("non-empty queue");
        if q.pq.is_empty() {
            // Ownership moves to the grantee; only QUEUED clears.
            state.store(LOCKED, Relaxed);
        }
        // SAFETY: queued waiters belong to clients borrowed by pending
        // futures (module docs), so the pointer is live.
        let node = unsafe { node_ptr.as_ref() };
        node.grant_base.store(base, Relaxed);
        if rec {
            let t_grant = cycles();
            node.grant_tsc.store(t_grant, Relaxed);
            node.grant_spin.store(t_locked.wrapping_sub(t_end), Relaxed);
            node.grant_queue.store(t_grant - t_locked, Relaxed);
        } else if self.opts.record_handoffs {
            node.grant_tsc.store(0, Relaxed);
        }
        // SAFETY: spinlock held.
        let waker = unsafe { (*node.waker.get()).take() };
        // Release pairs with the grantee's Acquire in `poll_granted`; it is
        // the last access to `node`.
        node.granted.store(true, Release);
        self.unlock_spin();
        waker
    }

    /// A queued request is dropped: unlink it, or, if ownership had been
    /// granted to it meanwhile, pass the lock on. Returns a waker to wake.
    fn cancel(&self, node: &Waiter) -> Option<Waker> {
        self.lock_spin();
        if node.granted.load(Acquire) {
            self.unlock_spin();
            return self.release(cycles());
        }
        // SAFETY: spinlock held.
        let q = unsafe { &mut *self.queue.get() };
        let removed = q.pq.remove(NonNull::from(node));
        debug_assert!(removed, "cancelled waiter was not queued");
        if q.pq.is_empty() {
            self.hot.state.fetch_and(!QUEUED, Relaxed);
        }
        // SAFETY: spinlock held, node unlinked.
        unsafe { *node.waker.get() = None };
        self.unlock_spin();
        None
    }
}

impl<T: Send + 'static> DelegationLock<T> for DispatchPq<T> {
    type Client = DispatchPqClient<T>;

    fn new(data: T) -> Self {
        Self::with_options(data, DispatchPqOptions::default())
    }

    fn client(self: &Arc<Self>) -> DispatchPqClient<T> {
        DispatchPqClient {
            lock: Arc::clone(self),
            // SAFETY: `Box::into_raw` never returns null.
            node: unsafe { NonNull::new_unchecked(Box::into_raw(Box::new(Waiter::new()))) },
        }
    }

    fn name() -> &'static str {
        "dispatch-pq"
    }
}

// ---------------------------------------------------------------------------
// Client and the `run` future
// ---------------------------------------------------------------------------

pub struct DispatchPqClient<T> {
    lock: Arc<DispatchPq<T>>,
    /// Owned (from `Box::into_raw`), freed in `Drop`. A raw pointer rather
    /// than a field so that `&mut self` never asserts uniqueness over memory
    /// a releaser may be reading.
    node: NonNull<Waiter>,
}

// SAFETY: the waiter is owned by the client; other threads reach it only
// through the queue protocol.
unsafe impl<T: Send> Send for DispatchPqClient<T> {}

impl<T> DispatchPqClient<T> {
    #[inline]
    fn waiter<'n>(node: NonNull<Waiter>) -> &'n Waiter {
        // SAFETY: the client keeps its waiter alive until it is dropped, and
        // no `run` future (which borrows the client) outlives the client.
        unsafe { node.as_ref() }
    }

    /// Test only: set this client's charged usage (counts as one service).
    #[cfg(test)]
    fn precharge(&self, usage: u64) {
        Self::waiter(self.node).charge(usage);
    }
}

impl<T> Drop for DispatchPqClient<T> {
    fn drop(&mut self) {
        // SAFETY: no future borrows the client any more, so the waiter is
        // unlinked; it came from `Box::into_raw` in `client()`.
        let node = unsafe { Box::from_raw(self.node.as_ptr()) };
        if self.lock.opts.record_handoffs {
            // SAFETY: the waiter is exclusively ours now.
            let s = unsafe { *node.stats.get() };
            if s != HandoffStats::default() {
                self.lock.stats.lock().add(&s);
            }
        }
    }
}

impl<T: Send + 'static> LockClient<T> for DispatchPqClient<T> {
    fn run<R, F>(&mut self, f: F) -> impl Future<Output = R> + Send
    where
        R: Send,
        F: FnOnce(&mut T) -> R + Send,
    {
        Run {
            client: self,
            f: Some(f),
            state: State::Init,
        }
    }

    fn usage(&self) -> u64 {
        Self::waiter(self.node).usage()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Init,
    Queued,
    Done,
}

struct Run<'a, T, F> {
    client: &'a mut DispatchPqClient<T>,
    f: Option<F>,
    state: State,
}

impl<T, F> Unpin for Run<'_, T, F> {}

impl<T, R, F> Run<'_, T, F>
where
    F: FnOnce(&mut T) -> R,
{
    /// Run the critical section as owner, charge it, hand off.
    fn critical(&mut self, how: Acquired) -> R {
        let f = self.f.take().expect("closure taken twice");
        let lock = &*self.client.lock;
        let node = DispatchPqClient::<T>::waiter(self.client.node);
        let base = match how {
            Acquired::Handoff => node.grant_base.load(Relaxed),
            Acquired::Fast | Acquired::Free => node.usage(),
        };
        let t0 = cycles();
        // SAFETY: we own the lock.
        let r = f(unsafe { &mut *lock.data.get() });
        let t1 = cycles();
        let cs = t1.wrapping_sub(t0);
        node.charge(base.wrapping_add(cs));
        lock.account(cs);
        self.state = State::Done;
        if let Some(w) = lock.release(t1) {
            executor::wake_with(lock.opts.wake_placement, &w);
        }
        if lock.recording() {
            node.record(how, t0);
        }
        r
    }
}

impl<T, R, F> Future for Run<'_, T, F>
where
    F: FnOnce(&mut T) -> R,
{
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<R> {
        let this = self.get_mut();
        let lock = &*this.client.lock;
        let node = DispatchPqClient::<T>::waiter(this.client.node);
        match this.state {
            State::Init => {
                if lock.try_lock_fast() {
                    return Poll::Ready(this.critical(Acquired::Fast));
                }
                let t_enq = lock.recording().then(cycles);
                if lock.enqueue_or_acquire(node, cx) {
                    return Poll::Ready(this.critical(Acquired::Free));
                }
                if let Some(t) = t_enq {
                    node.record_enqueue(cycles() - t);
                }
                this.state = State::Queued;
                Poll::Pending
            }
            State::Queued => {
                if lock.poll_granted(node, cx) {
                    Poll::Ready(this.critical(Acquired::Handoff))
                } else {
                    Poll::Pending
                }
            }
            State::Done => panic!("dispatch_pq::Run polled after completion"),
        }
    }
}

impl<T, F> Drop for Run<'_, T, F> {
    fn drop(&mut self) {
        if self.state == State::Queued {
            let lock = &*self.client.lock;
            if let Some(w) = lock.cancel(DispatchPqClient::<T>::waiter(self.client.node)) {
                executor::wake_with(lock.opts.wake_placement, &w);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Test hooks
// ---------------------------------------------------------------------------

#[cfg(test)]
impl<T> DispatchPq<T> {
    /// Take the lock from outside a `run` future (fast path only).
    fn hold(&self) -> bool {
        self.try_lock_fast()
    }

    /// Release a lock taken by `hold`, handing it to the queue minimum.
    fn release_held(&self) {
        if let Some(w) = self.release(cycles()) {
            executor::wake_with(self.opts.wake_placement, &w);
        }
    }

    /// The protected value; caller guarantees nobody owns the lock.
    fn data(&self) -> &T {
        // SAFETY: see above.
        unsafe { &*self.data.get() }
    }

    /// Stats folded in from dropped clients.
    fn handoff_stats(&self) -> HandoffStats {
        *self.stats.lock()
    }
}

#[cfg(test)]
mod tests {
    use super::super::fc::testing::{run_threads, spin_cycles, BoxFut};
    use super::*;
    use crate::executor::Executor;
    use crate::stats::TaskKind;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    /// Deadline for one real-executor run; a lost handoff hangs it.
    const DEADLINE: Duration = Duration::from_secs(60);
    /// Test-executor stall limit: nothing runnable this long = lost wakeup.
    const STALL: Duration = Duration::from_secs(5);

    fn opts(wake_placement: Placement, clamp: u64) -> DispatchPqOptions {
        let d = DispatchPqOptions::default();
        DispatchPqOptions {
            wake_placement,
            queue: FcPqOptions {
                starvation_clamp: clamp,
                ..d.queue
            },
            ..d
        }
    }

    /// Hold the lock, queue one request per client (client i precharged to
    /// `usages[i]`, queued in index order) whose critical section logs i,
    /// release, then poll round-robin with a no-op waker until all are done.
    /// Returns the service order.
    fn service_log(o: DispatchPqOptions, usages: &[u64]) -> Vec<usize> {
        let lock = Arc::new(DispatchPq::with_options(Vec::<usize>::new(), o));
        let mut clients: Vec<_> = usages
            .iter()
            .map(|&u| {
                let c = lock.client();
                c.precharge(u);
                c
            })
            .collect();
        assert!(lock.hold());
        let mut cx = Context::from_waker(Waker::noop());
        let mut runs: Vec<_> = clients
            .iter_mut()
            .enumerate()
            .map(|(i, c)| Some(Box::pin(c.run(move |log: &mut Vec<usize>| log.push(i)))))
            .collect();
        for r in runs.iter_mut().flatten() {
            assert!(r.as_mut().poll(&mut cx).is_pending(), "lock is held");
        }
        lock.release_held();
        let mut left = runs.len();
        while left > 0 {
            let before = left;
            for slot in &mut runs {
                if slot
                    .as_mut()
                    .is_some_and(|r| r.as_mut().poll(&mut cx).is_ready())
                {
                    *slot = None;
                    left -= 1;
                }
            }
            assert!(left < before, "no queued request was granted");
        }
        drop(runs);
        drop(clients);
        let log = lock.data().clone();
        assert!(lock.hold(), "lock left held after the last handoff");
        log
    }

    #[test]
    fn release_hands_ownership_to_the_min_usage_waiter() {
        let usages = [300, 100, 400, 200];
        for o in [DispatchPqOptions::default(), opts(Placement::Default, 0)] {
            assert_eq!(service_log(o, &usages), vec![1, 3, 0, 2]);
        }
    }

    /// Client 0 (usage 1e6) queues first, four cheap clients after it. With
    /// clamp c it is promoted at the first handoff after it has watched c + 1
    /// handoffs go to others: the clamp counts handoffs.
    #[test]
    fn starvation_clamp_counts_handoffs() {
        let usages = [1_000_000, 10, 20, 30, 40];
        let log = |clamp| service_log(opts(Placement::Default, clamp), &usages);
        assert_eq!(log(0), vec![1, 2, 3, 4, 0]);
        assert_eq!(log(1), vec![1, 2, 0, 3, 4]);
        assert_eq!(log(2), vec![1, 2, 3, 0, 4]);
    }

    #[test]
    fn dropping_a_queued_or_granted_request_passes_the_lock_on() {
        let lock = Arc::new(DispatchPq::new(Vec::<usize>::new()));
        let (mut a, mut b, mut c) = (lock.client(), lock.client(), lock.client());
        a.precharge(10);
        b.precharge(20);
        c.precharge(30);
        assert!(lock.hold());
        let mut cx = Context::from_waker(Waker::noop());
        let mut ra = Box::pin(a.run(|log: &mut Vec<usize>| log.push(0)));
        let mut rb = Box::pin(b.run(|log: &mut Vec<usize>| log.push(1)));
        let mut rc = Box::pin(c.run(|log: &mut Vec<usize>| log.push(2)));
        assert!(ra.as_mut().poll(&mut cx).is_pending());
        assert!(rb.as_mut().poll(&mut cx).is_pending());
        assert!(rc.as_mut().poll(&mut cx).is_pending());
        drop(ra); // queued: unlinked
        lock.release_held(); // grants b (min of b, c)
        drop(rb); // granted but never polled: passes the lock to c
        assert!(rc.as_mut().poll(&mut cx).is_ready());
        drop(rc);
        assert_eq!(*lock.data(), vec![2]);
        assert!(lock.hold(), "lock left held");
    }

    /// `threads` OS threads x `per_thread` clients x `ops` tiny critical
    /// sections with random gaps of up to `max_gap` cycles, on executors
    /// that poll only woken tasks: a waiter stranded in the queue stalls its
    /// executor (panic after `STALL`). Returns the lock's handoff stats.
    fn race(threads: usize, per_thread: usize, ops: u64, max_gap: u64) -> HandoffStats {
        let o = DispatchPqOptions {
            record_handoffs: true,
            // Clamp 1: nearly every handoff rekeys the heap.
            ..opts(Placement::Default, 1)
        };
        let lock = Arc::new(DispatchPq::with_options(0u64, o));
        let tasks: Vec<Vec<BoxFut>> = (0..threads)
            .map(|t| {
                (0..per_thread)
                    .map(|i| {
                        let mut c = lock.client();
                        let mut rng =
                            ((t * per_thread + i) as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                        Box::pin(async move {
                            for _ in 0..ops {
                                c.run(|n: &mut u64| {
                                    *n += 1;
                                    spin_cycles(50);
                                })
                                .await;
                                rng ^= rng << 13;
                                rng ^= rng >> 7;
                                rng ^= rng << 17;
                                spin_cycles(rng % (max_gap + 1));
                            }
                        }) as BoxFut
                    })
                    .collect()
            })
            .collect();
        run_threads(tasks, STALL);
        let total = (threads * per_thread) as u64 * ops;
        assert_eq!(*lock.data(), total);
        let s = lock.handoff_stats();
        assert_eq!(s.fast + s.free + s.handoffs, total, "{s:?}");
        s
    }

    /// Dense (3 clients per thread, short gaps): a convoy, nearly every
    /// release finds a waiter that has just set QUEUED. Sparse (1 client per
    /// thread, long gaps): the lock is mostly free, and a colliding
    /// requester often fails the CAS but finds the lock released by the time
    /// it holds the spinlock. Both sides of the release/enqueue race must
    /// occur, and no request may be stranded.
    #[test]
    fn no_lost_wakeup_across_release_enqueue_races() {
        // Handoff stats count only while recording; no other test reads it.
        stats::set_recording(true);
        let dense = race(4, 3, 20_000, 2_000);
        let sparse = race(4, 1, 20_000, 100_000);
        assert!(
            dense.handoffs > 0 && sparse.handoffs > 0,
            "{dense:?} {sparse:?}"
        );
        assert!(sparse.free > 0, "{sparse:?}");
    }

    /// Two cost classes (8:1) on 4 OS threads under pure usage order (clamp
    /// off): the cheap clients get more requests served and the charged
    /// usage evens out; FIFO would give equal request counts.
    #[test]
    fn cheaper_clients_are_served_more() {
        const LIGHT: u64 = 2_000;
        const HEAVY: u64 = 8 * LIGHT;
        let lock = Arc::new(DispatchPq::with_options(0u64, opts(Placement::Default, 0)));
        let stop = Arc::new(AtomicBool::new(false));
        let served = Arc::new(AtomicU64::new(0));
        let out = Arc::new(Mutex::new(Vec::new()));
        let tasks: Vec<Vec<BoxFut>> = (0..4)
            .map(|_| {
                [LIGHT, HEAVY]
                    .into_iter()
                    .map(|cs| {
                        let mut c = lock.client();
                        let (stop, served, out) = (stop.clone(), served.clone(), out.clone());
                        Box::pin(async move {
                            let mut ops = 0u64;
                            while !stop.load(Relaxed) {
                                c.run(move |_: &mut u64| spin_cycles(cs)).await;
                                ops += 1;
                                if served.fetch_add(1, Relaxed) + 1 >= 8_000 {
                                    stop.store(true, Relaxed);
                                }
                            }
                            out.lock().push((cs, ops, c.usage()));
                        }) as BoxFut
                    })
                    .collect()
            })
            .collect();
        run_threads(tasks, STALL);
        let out = out.lock();
        let sum = |cs: u64, f: fn(&(u64, u64, u64)) -> u64| {
            out.iter().filter(|r| r.0 == cs).map(f).sum::<u64>() as f64
        };
        let (light_ops, heavy_ops) = (sum(LIGHT, |r| r.1), sum(HEAVY, |r| r.1));
        let usage_ratio = sum(HEAVY, |r| r.2) / sum(LIGHT, |r| r.2);
        assert!(
            light_ops > 3.0 * heavy_ops,
            "light {light_ops} vs heavy {heavy_ops} ops: {out:?}"
        );
        assert!(
            (0.5..=2.0).contains(&usage_ratio),
            "heavy/light charged usage {usage_ratio:.2}: {out:?}"
        );
    }

    /// Protected value that detects overlapping critical sections.
    #[derive(Default)]
    struct Guarded {
        count: u64,
        inside: bool,
        overlaps: u64,
    }

    /// `clients` tasks x `ops` non-atomic read-modify-writes on a real
    /// executor (balance 31), client i spawned on worker i mod `workers`,
    /// random gaps of up to `max_gap` cycles between requests. Returns
    /// (count, overlaps, per-client usage).
    fn stress(
        o: DispatchPqOptions,
        workers: usize,
        clients: usize,
        ops: u64,
        cs: u64,
        max_gap: u64,
    ) -> (u64, u64, Vec<u64>) {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let lock = Arc::new(DispatchPq::with_options(Guarded::default(), o));
            let exec = Executor::with_balance_interval(workers, 31);
            let tasks: Vec<_> = (0..clients)
                .map(|i| {
                    let lock = Arc::clone(&lock);
                    exec.spawn_on(i % workers, TaskKind::Client, async move {
                        let mut c = lock.client();
                        let mut rng = (i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                        let mut last = 0;
                        for _ in 0..ops {
                            let seen = c
                                .run(move |g: &mut Guarded| {
                                    if g.inside {
                                        g.overlaps += 1;
                                    }
                                    g.inside = true;
                                    let before = g.count;
                                    spin_cycles(cs);
                                    g.count = before + 1;
                                    g.inside = false;
                                    g.count
                                })
                                .await;
                            assert!(seen > last, "result {seen} not after {last}");
                            last = seen;
                            rng ^= rng << 13;
                            rng ^= rng >> 7;
                            rng ^= rng << 17;
                            spin_cycles(rng % (max_gap + 1));
                        }
                        c.usage()
                    })
                })
                .collect();
            let usage = exec.block_on(async {
                let mut v = Vec::with_capacity(tasks.len());
                for t in tasks {
                    v.push(t.await);
                }
                v
            });
            let read = exec.spawn(TaskKind::Client, {
                let lock = Arc::clone(&lock);
                async move {
                    lock.client()
                        .run(|g: &mut Guarded| (g.count, g.overlaps))
                        .await
                }
            });
            let (count, overlaps) = exec.block_on(read);
            exec.shutdown();
            let _ = tx.send((count, overlaps, usage));
        });
        match rx.recv_timeout(DEADLINE) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => {
                panic!("run did not finish within {DEADLINE:?}: lost handoff or wake-up")
            }
            Err(RecvTimeoutError::Disconnected) => panic!("run panicked (see above)"),
        }
    }

    /// Every placement x clamp {default 16, 1 (rekey almost every handoff),
    /// off}; sustained (no gaps) and sparse (gaps up to 40 k cycles, so the
    /// queue drains and the fast path and free-under-spinlock path mix with
    /// handoffs).
    #[test]
    fn mutual_exclusion_and_completion_on_the_executor() {
        let cs = 200;
        for placement in [Placement::Default, Placement::Remote, Placement::Home] {
            for clamp in [DEFAULT_STARVATION_CLAMP, 1, 0] {
                for (clients, ops, max_gap) in [(16, 2_000, 0), (8, 1_500, 40_000)] {
                    let name = format!("{placement:?} clamp={clamp} gap={max_gap}");
                    let (count, overlaps, usage) =
                        stress(opts(placement, clamp), 4, clients, ops, cs, max_gap);
                    assert_eq!(overlaps, 0, "{name}: overlapping critical sections");
                    assert_eq!(
                        count,
                        clients as u64 * ops,
                        "{name}: lost or duplicated request"
                    );
                    for u in &usage {
                        assert!(*u >= ops * cs, "{name}: usage {u} below the spin floor");
                    }
                }
            }
        }
    }
}
