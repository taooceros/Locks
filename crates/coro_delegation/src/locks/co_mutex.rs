//! `co-fifo` / `co-pq`: coroutine-style async mutex (API:
//! [`crate::lock::AsyncMutex`], RESEARCH.md "API assumption (2026-09-30)").
//!
//! ```text
//! let mut g = h.lock().await;   // fast path: one CAS; else enqueue, Pending
//! g.insert(k, v);               // critical section: the task's own code
//! g.unlock().await;             // release, may step aside once
//! ```
//!
//! One type, [`CoMutex<T, Q>`], with the successor policy `Q`:
//! [`FifoList`] (`co-fifo`: intrusive FIFO) or [`UsageList`] (`co-pq`:
//! FC-PQ's [`UsageQueue`], minimum cumulative charged usage first, newcomer
//! init and starvation clamp as in `dispatch-pq`, clamp counted in handoffs).
//!
//! ## Release
//!
//! With no waiter the lock becomes free and `unlock()` is `Ready` at once.
//! Otherwise ownership passes to the waiter `Q` selects (the lock stays held)
//! and, per [`CoOptions`]:
//!
//! - `co-pq`: wake the grantee with its ordinary `Waker`, with no executor
//!   placement hint. Async release self-wakes and returns `Pending` once
//!   unless `StepAside::None` was requested. Both `Remote` and `Home` mean
//!   this ordinary yield for co-pq; neither chooses a worker. Synchronous
//!   drop only wakes the grantee. Inline/chain settings apply to co-fifo only.
//! - `co-fifo` CES release: the grantee is woken into this worker's run-next
//!   slot ([`executor::wake_inline`]) and the releaser *steps aside*: it
//!   reschedules itself and returns `Pending` once, so the grantee's critical
//!   section runs right after this poll, before the releaser's continuation.
//!   [`StepAside::Remote`]: injector + unpark one worker
//!   ([`executor::reschedule_self_remote`], as `ces`). [`StepAside::Home`]:
//!   the releaser's home inbox, which is this worker's (the executor's home
//!   is the worker that last polled the task), i.e. the back of this
//!   worker's local queue.
//! - Chain bound: if the releasing poll was itself inline-resumed and the
//!   worker's inline chain ([`executor::current_chain`]) has reached
//!   `chain_bound` handoffs, the grantee is woken with `break_placement`
//!   instead and the releaser continues (`Ready`): `ces`'s chain break.
//! - For `co-fifo`, [`StepAside::None`], and `Drop` of a guard or of a
//!   never-polled `unlock()` future: synchronous release, waking inline
//!   (like tokio's LIFO slot) and the releaser continues its poll, so the
//!   grantee runs only when that poll ends; the chain bound applies the same
//!   way. Drops are counted in [`HandleStats::sync_drops`].
//!
//! ## Lock word and memory model
//!
//! As in `dispatch_pq` (its module docs carry the full argument): `state` is
//! `UNLOCKED`, `LOCKED` or `LOCKED | QUEUED`; under the queue spinlock
//! `QUEUED` is set iff the queue is non-empty, and while it is set only
//! spinlock holders write `state`.
//!
//! - Fast acquire: CAS `UNLOCKED -> LOCKED` (Acquire). Fast release: CAS
//!   `LOCKED -> UNLOCKED` (Release); it fails iff an enqueuer set `QUEUED`.
//! - Enqueue under the spinlock: CAS loop that takes a free lock or sets
//!   `QUEUED` (AcqRel), then push. The releaser's failing fast CAS is
//!   Acquire, so it takes the spinlock only after the push: a release cannot
//!   miss a concurrent enqueue (no lost wakeup).
//! - Slow release under the spinlock: advance the handoff clock, pop, clear
//!   `QUEUED` if the queue is now empty, write the grant's accounting base,
//!   take the waker, `granted.store(true, Release)` (the releaser's last
//!   access to the node), release the spinlock, then wake. The grantee's
//!   `granted.load(Acquire)` orders the previous critical section before
//!   its own.
//!
//! ## Waiter nodes (no per-request allocation)
//!
//! The node lives inside the `lock()` future ([`Lock`]), which is `!Unpin`
//! and pinned inside the task while queued; the queue holds a raw pointer to
//! it only while it is linked. `PhantomPinned` also keeps the future's
//! `&mut` from asserting uniqueness over the node while a releaser reads it.
//! Dropping a queued `Lock` unlinks the node or, if ownership was granted
//! meanwhile, passes the lock on; leaking a pinned `Lock` is excluded (as in
//! `lock.rs`). The per-task usage account lives in the [`CoHandle`], which
//! every `Lock` of that handle borrows. Waker clones are reference-count
//! increments; the `UsageQueue` heap is preallocated.
//!
//! ## Accounting
//!
//! - Usage (co-pq's order; `usage()` for both): one request is charged
//!   `base + cs`, with `cs` = `cycles()` from `lock()` returning to `unlock()`
//!   being called (or the guard dropped), and `base` the usage at admission
//!   (a handoff carries it in the grant, possibly the newcomer init; a fast
//!   or free acquisition uses the handle's own usage), as in `dispatch-pq`.
//!   The base is read at grant for co-fifo (`FifoList::pop`) and at
//!   enqueue for co-pq (the heap key); these agree while a handle has one
//!   request at a time (the harness). With concurrent requests on one
//!   handle the later `charge` overwrites the earlier (one charge lost).
//!   The charge is *hold time*: any `.await` while holding counts.
//! - Burden: [`stats::record_foreign_cs`] (definition there), same `cs`,
//!   attributed to the worker where `lock()` returned; a guard held across
//!   an `.await` that migrates the task still charges that worker.

use std::cell::UnsafeCell;
use std::future::Future;
use std::marker::{PhantomData, PhantomPinned};
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::ptr::{self, NonNull};
use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use crossbeam_utils::CachePadded;
use serde::Serialize;

use super::fc_pq::{FcPqOptions, UsageNode, UsageQueue};
use crate::executor::{self, Placement};
use crate::lock::{cycles, AsyncGuard, AsyncMutex, CoLock};
use crate::stats;

/// Default chain bound K in handoffs (the bound of `ces-k64-home`).
pub const DEFAULT_CHAIN_BOUND: u32 = 64;

/// Default co-pq starvation clamp in handoffs: 16 is FIFO-degenerate for a
/// handoff clock, 256 is the fair setting (FINDINGS 2026-09-29 dispatch-pq).
pub const DEFAULT_STARVATION_CLAMP: u64 = 256;

const UNLOCKED: u8 = 0;
const LOCKED: u8 = 1;
const QUEUED: u8 = 2;

/// Worker id of a request made off the executor.
const NO_WORKER: usize = usize::MAX;

/// Where `unlock().await` puts the releaser after handing ownership on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StepAside {
    /// Injector + unpark one worker (`executor::reschedule_self_remote`).
    #[default]
    Remote,
    /// This worker's inbox, i.e. the back of its local queue.
    Home,
    /// No suspension: synchronous release, as `Drop`.
    None,
}

impl StepAside {
    pub fn label(self) -> &'static str {
        match self {
            StepAside::Remote => "remote",
            StepAside::Home => "home",
            StepAside::None => "none",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoOptions {
    pub step_aside: StepAside,
    /// Break the inline chain after this many handoffs (`None` = unbounded).
    pub chain_bound: Option<u32>,
    /// Wake placement of a chain-break handoff (`Inline` is treated as
    /// `Default`).
    pub break_placement: Placement,
    /// co-pq only: `starvation_clamp` (handoffs, 0 = off), `newcomer_init`,
    /// `record_waits` (waits in handoffs, via `fc_pq::take_wait_stats`). The
    /// combining knobs are unused.
    pub queue: FcPqOptions,
}

impl Default for CoOptions {
    fn default() -> Self {
        CoOptions {
            step_aside: StepAside::Remote,
            chain_bound: Some(DEFAULT_CHAIN_BOUND),
            break_placement: Placement::Home,
            queue: FcPqOptions {
                starvation_clamp: DEFAULT_STARVATION_CLAMP,
                ..FcPqOptions::default()
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Waiter node and per-handle account
// ---------------------------------------------------------------------------

/// Cumulative charge of one handle. Written only by the current lock owner
/// (after its critical section), read by enqueuers under the spinlock.
#[derive(Default)]
struct Account {
    usage: AtomicU64,
    served: AtomicU64,
}

impl Account {
    fn charge(&self, usage: u64) {
        self.usage.store(usage, Relaxed);
        self.served.store(self.served.load(Relaxed) + 1, Relaxed);
    }
}

/// Queue entry, embedded in a [`Lock`] future.
#[repr(C, align(128))]
pub struct Waiter {
    /// Guarded by the queue spinlock.
    waker: UnsafeCell<Option<Waker>>,
    /// Release by the granting releaser (under the spinlock), Acquire by the
    /// waiter.
    granted: AtomicBool,
    /// Accounting base of the grant, written before the `granted` Release.
    grant_base: AtomicU64,
    /// [`FifoList`] link; guarded by the queue spinlock.
    next: UnsafeCell<*const Waiter>,
    /// Owning handle's account; the `Lock` future borrows the handle, so it
    /// outlives the node.
    account: NonNull<Account>,
    /// Worker polling the task when it queued (`NO_WORKER` off-executor).
    /// Written by the owner before the node is published.
    home: usize,
}

// SAFETY: `waker` and `next` are touched only under the queue spinlock (or
// by the owner before publication); `account` points to atomics of a handle
// that outlives the node; everything else is atomic or owner-only.
unsafe impl Send for Waiter {}
unsafe impl Sync for Waiter {}

impl Waiter {
    fn new(account: &Account) -> Self {
        Waiter {
            waker: UnsafeCell::new(None),
            granted: AtomicBool::new(false),
            grant_base: AtomicU64::new(0),
            next: UnsafeCell::new(ptr::null()),
            account: NonNull::from(account),
            home: NO_WORKER,
        }
    }

    #[inline]
    fn account(&self) -> &Account {
        // SAFETY: see the `account` field.
        unsafe { self.account.as_ref() }
    }
}

impl UsageNode for Waiter {
    fn usage(&self) -> u64 {
        self.account().usage.load(Relaxed)
    }
    fn served(&self) -> u64 {
        self.account().served.load(Relaxed)
    }
    fn charge(&self, usage: u64) {
        self.account().charge(usage);
    }
    fn set_usage(&self, usage: u64) {
        self.account().usage.store(usage, Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Successor policies
// ---------------------------------------------------------------------------

/// Waiter queue of a [`CoMutex`]. Every call is made under the queue
/// spinlock. The tick is the handoff count (the starvation clamp's clock).
pub trait WaitList: Send + 'static {
    const NAME: &'static str;
    /// Whether the lock must maintain running usage totals (newcomer init).
    const USAGE_ORDERED: bool;
    fn new(opts: FcPqOptions) -> Self;
    fn is_empty(&self) -> bool;
    /// Admit `w` at tick `tick`; `totals` = lifetime (charged usage,
    /// requests served) of the lock, for the newcomer init.
    fn push(&mut self, w: NonNull<Waiter>, tick: u64, totals: (u64, u64));
    /// Remove the next owner at handoff `tick`, with its accounting base.
    fn pop(&mut self, tick: u64) -> Option<(NonNull<Waiter>, u64)>;
    /// Unlink a queued `w` (cancellation); whether it was queued.
    fn remove(&mut self, w: NonNull<Waiter>) -> bool;
}

/// `co-fifo`: intrusive FIFO over [`Waiter::next`].
pub struct FifoList {
    head: *const Waiter,
    tail: *const Waiter,
}

// SAFETY: the pointers are dereferenced only by the spinlock holder.
unsafe impl Send for FifoList {}

impl WaitList for FifoList {
    const NAME: &'static str = "co-fifo";
    const USAGE_ORDERED: bool = false;

    fn new(_: FcPqOptions) -> Self {
        FifoList {
            head: ptr::null(),
            tail: ptr::null(),
        }
    }

    fn is_empty(&self) -> bool {
        self.head.is_null()
    }

    fn push(&mut self, w: NonNull<Waiter>, _tick: u64, _totals: (u64, u64)) {
        let p = w.as_ptr() as *const Waiter;
        // SAFETY: `w` is live and unlinked; `tail` is live and linked;
        // spinlock held.
        unsafe {
            *(*p).next.get() = ptr::null();
            if self.tail.is_null() {
                self.head = p;
            } else {
                *(*self.tail).next.get() = p;
            }
        }
        self.tail = p;
    }

    fn pop(&mut self, _tick: u64) -> Option<(NonNull<Waiter>, u64)> {
        let h = self.head;
        if h.is_null() {
            return None;
        }
        // SAFETY: `h` is a live linked node; spinlock held.
        unsafe {
            self.head = *(*h).next.get();
            if self.head.is_null() {
                self.tail = ptr::null();
            }
            Some((NonNull::from(&*h), (*h).usage()))
        }
    }

    fn remove(&mut self, w: NonNull<Waiter>) -> bool {
        let target = w.as_ptr() as *const Waiter;
        let mut prev: *const Waiter = ptr::null();
        let mut cur = self.head;
        // SAFETY: every node on the list is live; spinlock held.
        unsafe {
            while !cur.is_null() && cur != target {
                prev = cur;
                cur = *(*cur).next.get();
            }
            if cur.is_null() {
                return false;
            }
            let next = *(*cur).next.get();
            if prev.is_null() {
                self.head = next;
            } else {
                *(*prev).next.get() = next;
            }
            if self.tail == target {
                self.tail = prev;
            }
        }
        true
    }
}

/// `co-pq`: FC-PQ's usage queue (shared with `dispatch-pq`), ticked by
/// handoffs: the clamp runs at every handoff, before the pop.
pub struct UsageList(UsageQueue<Waiter>);

impl WaitList for UsageList {
    const NAME: &'static str = "co-pq";
    const USAGE_ORDERED: bool = true;

    fn new(opts: FcPqOptions) -> Self {
        UsageList(UsageQueue::new(opts))
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn push(&mut self, w: NonNull<Waiter>, tick: u64, (usage, served): (u64, u64)) {
        // SAFETY: `w` is live (its `Lock` is pending).
        if unsafe { w.as_ref() }.served() == 0 {
            self.0.set_totals(usage, served);
        }
        self.0.push(w, tick);
    }

    fn pop(&mut self, tick: u64) -> Option<(NonNull<Waiter>, u64)> {
        self.0.promote_starving(tick);
        self.0.pop()
    }

    fn remove(&mut self, w: NonNull<Waiter>) -> bool {
        self.0.remove(w)
    }
}

// ---------------------------------------------------------------------------
// Lock
// ---------------------------------------------------------------------------

/// Lock word, queue spinlock and the newcomer-init totals: every acquire
/// and release touches this line.
struct Hot {
    state: AtomicU8,
    spin: AtomicBool,
    /// Lifetime charged cycles / requests served; owner-written (co-pq only).
    usage_total: AtomicU64,
    served_total: AtomicU64,
}

/// Spinlock-guarded.
struct Queue<Q> {
    list: Q,
    /// Handoffs so far: the starvation clamp's clock.
    handoffs: u64,
}

pub struct CoMutex<T, Q: WaitList> {
    hot: CachePadded<Hot>,
    queue: CachePadded<UnsafeCell<Queue<Q>>>,
    data: UnsafeCell<T>,
    opts: CoOptions,
}

pub type CoFifo<T> = CoMutex<T, FifoList>;
pub type CoPq<T> = CoMutex<T, UsageList>;

// SAFETY: `queue` is accessed only under `hot.spin`; `data` only by the lock
// owner; queued raw pointers refer to nodes of pending `Lock` futures.
unsafe impl<T: Send, Q: WaitList> Send for CoMutex<T, Q> {}
unsafe impl<T: Send, Q: WaitList> Sync for CoMutex<T, Q> {}

impl<T, Q: WaitList> CoMutex<T, Q> {
    pub fn with_options(data: T, opts: CoOptions) -> Self {
        let opts = CoOptions {
            break_placement: match opts.break_placement {
                Placement::Inline => Placement::Default,
                p => p,
            },
            ..opts
        };
        CoMutex {
            hot: CachePadded::new(Hot {
                state: AtomicU8::new(UNLOCKED),
                spin: AtomicBool::new(false),
                usage_total: AtomicU64::new(0),
                served_total: AtomicU64::new(0),
            }),
            queue: CachePadded::new(UnsafeCell::new(Queue {
                list: Q::new(opts.queue),
                handoffs: 0,
            })),
            data: UnsafeCell::new(data),
            opts,
        }
    }

    pub fn options(&self) -> CoOptions {
        self.opts
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

    /// Slow acquire: `true` if the lock was free (now ours); otherwise
    /// `node` is queued with `cx`'s waker.
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
        // SAFETY: spinlock held; the node is not linked yet.
        unsafe { *node.waker.get() = Some(cx.waker().clone()) };
        let totals = if Q::USAGE_ORDERED {
            (
                self.hot.usage_total.load(Relaxed),
                self.hot.served_total.load(Relaxed),
            )
        } else {
            (0, 0)
        };
        // SAFETY: spinlock held.
        let q = unsafe { &mut *self.queue.get() };
        q.list.push(NonNull::from(node), q.handoffs + 1, totals);
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
    fn account_totals(&self, cs: u64) {
        let h = &*self.hot;
        h.usage_total
            .store(h.usage_total.load(Relaxed).wrapping_add(cs), Relaxed);
        h.served_total
            .store(h.served_total.load(Relaxed) + 1, Relaxed);
    }

    /// Owner only: release. With waiters, ownership passes to the one `Q`
    /// selects and its waker is returned for the caller to wake.
    fn release(&self) -> Option<Waker> {
        let state = &self.hot.state;
        if state
            .compare_exchange(LOCKED, UNLOCKED, Release, Acquire)
            .is_ok()
        {
            return None;
        }
        self.lock_spin();
        // SAFETY: spinlock held.
        let q = unsafe { &mut *self.queue.get() };
        if q.list.is_empty() {
            // Every waiter cancelled after our CAS saw QUEUED; `cancel`
            // already cleared the bit.
            state.store(UNLOCKED, Release);
            self.unlock_spin();
            return None;
        }
        q.handoffs += 1;
        let (w, base) = q.list.pop(q.handoffs).expect("non-empty queue");
        if q.list.is_empty() {
            // Ownership moves to the grantee; only QUEUED clears.
            state.store(LOCKED, Relaxed);
        }
        // SAFETY: queued nodes belong to pending `Lock` futures (module
        // docs), so the pointer is live.
        let node = unsafe { w.as_ref() };
        node.grant_base.store(base, Relaxed);
        // SAFETY: spinlock held.
        let waker = unsafe { (*node.waker.get()).take() };
        // Every linked node carries a waker (set before the push, cleared
        // only after unlinking); `None` here would be a lost wakeup.
        debug_assert!(waker.is_some(), "granted waiter without a waker");
        // Release pairs with the grantee's Acquire in `poll_granted`; last
        // access to `node`.
        node.granted.store(true, Release);
        self.unlock_spin();
        waker
    }

    /// A queued `Lock` is dropped: unlink its node, or, if ownership had
    /// been granted meanwhile, pass the lock on. Returns a waker to wake.
    fn cancel(&self, node: &Waiter) -> Option<Waker> {
        self.lock_spin();
        if node.granted.load(Acquire) {
            self.unlock_spin();
            return self.release();
        }
        // SAFETY: spinlock held.
        let q = unsafe { &mut *self.queue.get() };
        let removed = q.list.remove(NonNull::from(node));
        debug_assert!(removed, "cancelled waiter was not queued");
        if q.list.is_empty() {
            self.hot.state.fetch_and(!QUEUED, Relaxed);
        }
        // SAFETY: spinlock held, node unlinked.
        unsafe { *node.waker.get() = None };
        self.unlock_spin();
        None
    }

    /// Wake the grantee of a slow release. With `cx` (an `unlock()` poll)
    /// and a stepping policy, also reschedule the current task; the caller
    /// must then return `Pending` right away (returns `true`).
    fn place_grantee(&self, waker: &Waker, cx: Option<&Context<'_>>, st: &Counters) -> bool {
        if Q::USAGE_ORDERED {
            waker.wake_by_ref();
            if let Some(cx) = cx {
                if self.opts.step_aside != StepAside::None {
                    cx.waker().wake_by_ref();
                    Counters::bump(&st.step_asides);
                    return true;
                }
            }
            return false;
        } else if self.chain_bound_reached() {
            Counters::bump(&st.chain_breaks);
            executor::wake_with(self.opts.break_placement, waker);
            return false;
        } else {
            executor::wake_inline(waker);
        }
        match (cx, self.opts.step_aside) {
            (Some(cx), StepAside::Remote) => {
                executor::reschedule_self_remote(cx);
            }
            (Some(cx), StepAside::Home) => {
                // As `reschedule_self_remote`, with the `Home` hint: the
                // deferred schedule runs on this thread after `Pending` and
                // finds this worker as the task's home.
                executor::set_placement_hint(Placement::Home);
                cx.waker().wake_by_ref();
            }
            _ => return false,
        }
        Counters::bump(&st.step_asides);
        true
    }

    /// True when this release happens inside an inline chain that has
    /// reached `chain_bound` handoffs (as `ces`).
    fn chain_bound_reached(&self) -> bool {
        let Some(k) = self.opts.chain_bound else {
            return false;
        };
        let (len, _) = executor::current_chain();
        len != 0 && len >= k
    }
}

impl<T: Send + 'static, Q: WaitList> CoLock<T> for CoMutex<T, Q> {
    type Handle = CoHandle<T, Q>;

    fn new(data: T) -> Self {
        Self::with_options(data, CoOptions::default())
    }

    fn handle(self: &Arc<Self>) -> CoHandle<T, Q> {
        CoHandle {
            mutex: Arc::clone(self),
            account: Account::default(),
            stats: Counters::default(),
        }
    }

    fn name() -> &'static str {
        Q::NAME
    }
}

// ---------------------------------------------------------------------------
// Handle
// ---------------------------------------------------------------------------

/// Per-handle counters over the handle's lifetime. `fast` / `free` /
/// `handoff`: how `lock()` obtained ownership (uncontended CAS; found free
/// under the spinlock; granted by a releaser). `step_asides`: `unlock()`
/// calls that returned `Pending` once. `chain_breaks`: releases that woke
/// the grantee with the break placement. `sync_drops`: releases by `Drop`
/// (guard dropped without `unlock()`, or `unlock()` future never polled).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct HandleStats {
    pub fast: u64,
    pub free: u64,
    pub handoff: u64,
    pub step_asides: u64,
    pub chain_breaks: u64,
    pub sync_drops: u64,
}

impl HandleStats {
    pub fn add(&mut self, o: &HandleStats) {
        self.fast += o.fast;
        self.free += o.free;
        self.handoff += o.handoff;
        self.step_asides += o.step_asides;
        self.chain_breaks += o.chain_breaks;
        self.sync_drops += o.sync_drops;
    }

    /// `self - earlier`, field by field.
    pub fn since(&self, earlier: &HandleStats) -> HandleStats {
        HandleStats {
            fast: self.fast - earlier.fast,
            free: self.free - earlier.free,
            handoff: self.handoff - earlier.handoff,
            step_asides: self.step_asides - earlier.step_asides,
            chain_breaks: self.chain_breaks - earlier.chain_breaks,
            sync_drops: self.sync_drops - earlier.sync_drops,
        }
    }
}

/// Written only by the lock owner of the moment (serialised by the lock).
#[derive(Default)]
struct Counters {
    fast: AtomicU64,
    free: AtomicU64,
    handoff: AtomicU64,
    step_asides: AtomicU64,
    chain_breaks: AtomicU64,
    sync_drops: AtomicU64,
}

impl Counters {
    #[inline]
    fn bump(c: &AtomicU64) {
        c.store(c.load(Relaxed) + 1, Relaxed);
    }
}

/// A task's handle: its usage account and counters (see
/// [`crate::lock::AsyncMutex`] for why the API sits on a handle).
pub struct CoHandle<T, Q: WaitList> {
    mutex: Arc<CoMutex<T, Q>>,
    account: Account,
    stats: Counters,
}

impl<T, Q: WaitList> CoHandle<T, Q> {
    /// Acquire. Holding the returned guard across other `.await`s is fine.
    pub fn lock(&self) -> Lock<'_, T, Q> {
        Lock {
            handle: self,
            state: LockState::Init,
            node: Waiter::new(&self.account),
            _pin: PhantomPinned,
        }
    }

    pub fn stats(&self) -> HandleStats {
        let c = &self.stats;
        HandleStats {
            fast: c.fast.load(Relaxed),
            free: c.free.load(Relaxed),
            handoff: c.handoff.load(Relaxed),
            step_asides: c.step_asides.load(Relaxed),
            chain_breaks: c.chain_breaks.load(Relaxed),
            sync_drops: c.sync_drops.load(Relaxed),
        }
    }

    /// Owner path of `lock()`: build the guard.
    fn acquired(&self, how: &AtomicU64, base: u64, home: usize) -> Guard<'_, T, Q> {
        Counters::bump(how);
        let worker = executor::worker_id().unwrap_or(NO_WORKER);
        Guard {
            held: Held {
                handle: self,
                base,
                t_acq: cycles(),
                worker,
                foreign: home != NO_WORKER && worker != NO_WORKER && home != worker,
            },
            _data: PhantomData,
        }
    }
}

impl<T: Send + 'static, Q: WaitList> AsyncMutex<T> for CoHandle<T, Q> {
    type Guard<'a>
        = Guard<'a, T, Q>
    where
        Self: 'a;

    fn lock(&self) -> impl Future<Output = Guard<'_, T, Q>> + Send {
        CoHandle::lock(self)
    }

    fn usage(&self) -> u64 {
        self.account.usage.load(Relaxed)
    }
}

// ---------------------------------------------------------------------------
// lock() future
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum LockState {
    Init,
    Queued,
    Done,
}

/// Future of [`CoHandle::lock`]; carries the intrusive waiter node.
pub struct Lock<'a, T, Q: WaitList> {
    handle: &'a CoHandle<T, Q>,
    state: LockState,
    node: Waiter,
    _pin: PhantomPinned,
}

impl<'a, T, Q: WaitList> Future for Lock<'a, T, Q> {
    type Output = Guard<'a, T, Q>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Guard<'a, T, Q>> {
        // SAFETY: nothing is moved out of the pinned future. `node` is
        // written through this `&mut` only before it is published (Init);
        // after publication only `state` (other bytes) is written. For a
        // `!Unpin` type the `&mut` asserts no uniqueness (Stacked Borrows
        // gives it SharedReadWrite, as for tokio's intrusive futures), so a
        // releaser's concurrent `&Waiter` stays valid; never form a
        // `&mut Waiter` after publication.
        let this = unsafe { self.get_unchecked_mut() };
        let h = this.handle;
        let m = &*h.mutex;
        match this.state {
            LockState::Init => {
                let own = h.account.usage.load(Relaxed);
                if m.try_lock_fast() {
                    this.state = LockState::Done;
                    return Poll::Ready(h.acquired(&h.stats.fast, own, NO_WORKER));
                }
                this.node.home = executor::worker_id().unwrap_or(NO_WORKER);
                if m.enqueue_or_acquire(&this.node, cx) {
                    this.state = LockState::Done;
                    return Poll::Ready(h.acquired(&h.stats.free, own, NO_WORKER));
                }
                this.state = LockState::Queued;
                Poll::Pending
            }
            LockState::Queued => {
                if m.poll_granted(&this.node, cx) {
                    this.state = LockState::Done;
                    let base = this.node.grant_base.load(Relaxed);
                    Poll::Ready(h.acquired(&h.stats.handoff, base, this.node.home))
                } else {
                    Poll::Pending
                }
            }
            LockState::Done => panic!("co_mutex::Lock polled after completion"),
        }
    }
}

impl<T, Q: WaitList> Drop for Lock<'_, T, Q> {
    fn drop(&mut self) {
        if self.state == LockState::Queued {
            if let Some(w) = self.handle.mutex.cancel(&self.node) {
                w.wake();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Guard and unlock() future
// ---------------------------------------------------------------------------

/// What the owner knows about its critical section.
struct Held<'a, T, Q: WaitList> {
    handle: &'a CoHandle<T, Q>,
    /// Accounting base of this request.
    base: u64,
    /// `cycles()` when `lock()` returned.
    t_acq: u64,
    /// Worker where `lock()` returned.
    worker: usize,
    /// The request queued on another worker (burden definition).
    foreign: bool,
}

impl<T, Q: WaitList> Clone for Held<'_, T, Q> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T, Q: WaitList> Copy for Held<'_, T, Q> {}

impl<T, Q: WaitList> Held<'_, T, Q> {
    /// Owner: charge the critical section that ends now.
    fn charge(&self) {
        let cs = cycles().wrapping_sub(self.t_acq);
        self.handle.account.charge(self.base.wrapping_add(cs));
        if Q::USAGE_ORDERED {
            self.handle.mutex.account_totals(cs);
        }
        if self.foreign {
            stats::record_foreign_cs(self.worker, cs);
        }
    }

    /// Synchronous release (`Drop`).
    fn release_sync(&self) {
        let h = self.handle;
        Counters::bump(&h.stats.sync_drops);
        self.charge();
        if let Some(w) = h.mutex.release() {
            h.mutex.place_grantee(&w, None, &h.stats);
        }
    }
}

/// Exclusive access to the protected value. Release with
/// [`Guard::unlock`]`.await`; dropping it releases synchronously.
///
/// `Send` iff `T: Send`, `Sync` iff `T: Send + Sync` (like
/// `&mut T`): a shared `&Guard` hands out `&T` on every thread holding it,
/// so a `!Sync` payload must not make the guard `Sync`.
///
/// ```
/// use std::cell::Cell;
/// use coro_delegation::locks::co_mutex::{FifoList, Guard};
/// fn send<S: Send>() {}
/// fn sync<S: Sync>() {}
/// sync::<Guard<'static, u64, FifoList>>();
/// send::<Guard<'static, Cell<u64>, FifoList>>();
/// ```
///
/// ```compile_fail,E0277
/// use std::cell::Cell;
/// use coro_delegation::locks::co_mutex::{FifoList, Guard};
/// fn sync<S: Sync>() {}
/// sync::<Guard<'static, Cell<u64>, FifoList>>();
/// ```
pub struct Guard<'a, T, Q: WaitList> {
    held: Held<'a, T, Q>,
    /// `Held` reaches `T` only through `&CoHandle`, which is `Sync` for
    /// `T: Send`; this marker adds the `&mut T` auto-trait bounds.
    _data: PhantomData<&'a mut T>,
}

impl<'a, T, Q: WaitList> Guard<'a, T, Q> {
    /// Explicit release; see the module docs for what the returned future
    /// does with the releasing task.
    pub fn unlock(self) -> Unlock<'a, T, Q> {
        let held = self.held;
        std::mem::forget(self);
        Unlock {
            held: Some(held),
            stepped: false,
        }
    }
}

impl<T, Q: WaitList> Deref for Guard<'_, T, Q> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard owns the lock.
        unsafe { &*self.held.handle.mutex.data.get() }
    }
}

impl<T, Q: WaitList> DerefMut for Guard<'_, T, Q> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard owns the lock, and `&mut self` is unique.
        unsafe { &mut *self.held.handle.mutex.data.get() }
    }
}

impl<T, Q: WaitList> Drop for Guard<'_, T, Q> {
    fn drop(&mut self) {
        self.held.release_sync();
    }
}

impl<T: Send + 'static, Q: WaitList> AsyncGuard<T> for Guard<'_, T, Q> {
    fn unlock(self) -> impl Future<Output = ()> + Send {
        Guard::unlock(self)
    }
}

/// Future of [`Guard::unlock`]: releases at its first poll and returns
/// `Pending` once if it stepped aside. Dropped unpolled, it releases
/// synchronously.
pub struct Unlock<'a, T, Q: WaitList> {
    held: Option<Held<'a, T, Q>>,
    stepped: bool,
}

impl<T, Q: WaitList> Future for Unlock<'_, T, Q> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        let Some(held) = this.held.take() else {
            assert!(this.stepped, "co_mutex::Unlock polled after completion");
            this.stepped = false;
            return Poll::Ready(());
        };
        held.charge();
        let h = held.handle;
        match h.mutex.release() {
            Some(w) if h.mutex.place_grantee(&w, Some(cx), &h.stats) => {
                this.stepped = true;
                Poll::Pending
            }
            _ => Poll::Ready(()),
        }
    }
}

impl<T, Q: WaitList> Drop for Unlock<'_, T, Q> {
    fn drop(&mut self) {
        if let Some(held) = self.held.take() {
            held.release_sync();
        }
    }
}

// ---------------------------------------------------------------------------
// Test hooks
// ---------------------------------------------------------------------------

#[cfg(test)]
impl<T, Q: WaitList> CoHandle<T, Q> {
    /// Set this handle's charged usage (counts as one service).
    fn precharge(&self, usage: u64) {
        self.account.charge(usage);
    }
}

#[cfg(test)]
impl<T, Q: WaitList> CoMutex<T, Q> {
    /// The protected value; caller guarantees nobody owns the lock.
    fn data_ref(&self) -> &T {
        // SAFETY: see above.
        unsafe { &*self.data.get() }
    }

    fn is_free(&self) -> bool {
        self.hot.state.load(Acquire) == UNLOCKED
    }
}

#[cfg(test)]
mod tests {
    use super::super::fc::testing::{run_threads, spin_cycles, BoxFut};
    use super::*;
    use crate::executor::{yield_now, Executor};
    use crate::stats::TaskKind;
    use parking_lot::Mutex;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    /// Deadline for one real-executor run; a lost handoff hangs it.
    const DEADLINE: Duration = Duration::from_secs(60);
    /// Test-executor stall limit: nothing runnable this long = lost wakeup.
    const STALL: Duration = Duration::from_secs(5);

    fn opts(step_aside: StepAside, chain_bound: Option<u32>, clamp: u64) -> CoOptions {
        let d = CoOptions::default();
        CoOptions {
            step_aside,
            chain_bound,
            queue: FcPqOptions {
                starvation_clamp: clamp,
                ..d.queue
            },
            ..d
        }
    }

    const STEPS: [StepAside; 3] = [StepAside::Remote, StepAside::Home, StepAside::None];

    /// Hold the lock, queue one `lock()` per handle (handle i precharged to
    /// `usages[i]`, queued in index order), release, then poll round-robin
    /// with a no-op waker; each granted request logs i and drops its guard.
    /// Returns the service order.
    fn service_log<Q: WaitList>(o: CoOptions, usages: &[u64]) -> Vec<usize> {
        let lock = Arc::new(CoMutex::<Vec<usize>, Q>::with_options(Vec::new(), o));
        let holder = lock.handle();
        let handles: Vec<_> = usages
            .iter()
            .map(|&u| {
                let h = lock.handle();
                h.precharge(u);
                h
            })
            .collect();
        let mut cx = Context::from_waker(Waker::noop());
        let mut g = Box::pin(holder.lock());
        let Poll::Ready(g) = g.as_mut().poll(&mut cx) else {
            panic!("uncontended lock() must be ready");
        };
        let mut pending: Vec<_> = handles.iter().map(|h| Some(Box::pin(h.lock()))).collect();
        for l in pending.iter_mut().flatten() {
            assert!(l.as_mut().poll(&mut cx).is_pending(), "lock is held");
        }
        drop(g); // synchronous release: grants the first waiter
        let mut left = pending.len();
        while left > 0 {
            let before = left;
            for (i, slot) in pending.iter_mut().enumerate() {
                let Some(l) = slot else { continue };
                if let Poll::Ready(mut g) = l.as_mut().poll(&mut cx) {
                    g.push(i);
                    drop(g);
                    *slot = None;
                    left -= 1;
                }
            }
            assert!(left < before, "no queued request was granted");
        }
        assert!(lock.is_free(), "lock left held after the last handoff");
        lock.data_ref().clone()
    }

    #[test]
    fn release_hands_ownership_in_policy_order() {
        let usages = [300, 100, 400, 200];
        for step in STEPS {
            assert_eq!(
                service_log::<FifoList>(opts(step, None, 0), &usages),
                vec![0, 1, 2, 3]
            );
            for clamp in [DEFAULT_STARVATION_CLAMP, 0] {
                assert_eq!(
                    service_log::<UsageList>(opts(step, None, clamp), &usages),
                    vec![1, 3, 0, 2]
                );
            }
        }
    }

    /// Handle 0 (usage 1e6) queues first, four cheap handles after it. With
    /// clamp c it is promoted at the first handoff after it has watched c + 1
    /// handoffs go to others (same schedule as `dispatch-pq`).
    #[test]
    fn starvation_clamp_counts_handoffs() {
        let usages = [1_000_000, 10, 20, 30, 40];
        let log = |clamp| service_log::<UsageList>(opts(StepAside::Remote, None, clamp), &usages);
        assert_eq!(log(0), vec![1, 2, 3, 4, 0]);
        assert_eq!(log(1), vec![1, 2, 0, 3, 4]);
        assert_eq!(log(2), vec![1, 2, 3, 0, 4]);
    }

    fn cancel_passes_the_lock_on<Q: WaitList>() {
        let lock = Arc::new(CoMutex::<Vec<usize>, Q>::with_options(
            Vec::new(),
            opts(StepAside::Remote, None, 0),
        ));
        let (h0, a, b, c) = (lock.handle(), lock.handle(), lock.handle(), lock.handle());
        a.precharge(10);
        b.precharge(20);
        c.precharge(30);
        let mut cx = Context::from_waker(Waker::noop());
        let Poll::Ready(g) = Box::pin(h0.lock()).as_mut().poll(&mut cx) else {
            panic!("uncontended");
        };
        let mut la = Box::pin(a.lock());
        let mut lb = Box::pin(b.lock());
        let mut lc = Box::pin(c.lock());
        assert!(la.as_mut().poll(&mut cx).is_pending());
        assert!(lb.as_mut().poll(&mut cx).is_pending());
        assert!(lc.as_mut().poll(&mut cx).is_pending());
        drop(la); // queued: unlinked
        drop(g); // grants b (FIFO head after a's removal; min usage of b, c)
        drop(lb); // granted but never polled: passes the lock to c
        let Poll::Ready(mut g) = lc.as_mut().poll(&mut cx) else {
            panic!("c must own the lock now");
        };
        g.push(2);
        drop(g);
        drop(lc);
        assert_eq!(*lock.data_ref(), vec![2]);
        assert!(lock.is_free(), "lock left held");
    }

    #[test]
    fn dropping_a_queued_or_granted_lock_future_passes_the_lock_on() {
        cancel_passes_the_lock_on::<FifoList>();
        cancel_passes_the_lock_on::<UsageList>();
    }

    /// `threads` OS threads x `per_thread` handles x `ops` tiny critical
    /// sections with random gaps of up to `max_gap` cycles, on executors that
    /// poll only woken tasks: a waiter stranded in the queue stalls its
    /// executor (panic after `STALL`). Returns the summed handle stats.
    fn race<Q: WaitList>(
        o: CoOptions,
        threads: usize,
        per: usize,
        ops: u64,
        max_gap: u64,
    ) -> HandleStats {
        let lock = Arc::new(CoMutex::<u64, Q>::with_options(0, o));
        let out = Arc::new(Mutex::new(HandleStats::default()));
        let tasks: Vec<Vec<BoxFut>> = (0..threads)
            .map(|t| {
                (0..per)
                    .map(|i| {
                        let lock = Arc::clone(&lock);
                        let out = Arc::clone(&out);
                        let mut rng =
                            ((t * per + i) as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                        Box::pin(async move {
                            let h = lock.handle();
                            for _ in 0..ops {
                                let mut g = h.lock().await;
                                *g += 1;
                                spin_cycles(50);
                                g.unlock().await;
                                rng ^= rng << 13;
                                rng ^= rng >> 7;
                                rng ^= rng << 17;
                                spin_cycles(rng % (max_gap + 1));
                            }
                            out.lock().add(&h.stats());
                        }) as BoxFut
                    })
                    .collect()
            })
            .collect();
        run_threads(tasks, STALL);
        let total = (threads * per) as u64 * ops;
        assert_eq!(*lock.data_ref(), total);
        assert!(lock.is_free());
        let s = *out.lock();
        assert_eq!(s.fast + s.free + s.handoff, total, "{s:?}");
        s
    }

    /// Dense (3 handles per thread, short gaps): nearly every unlock finds a
    /// waiter that has just set QUEUED. Sparse (1 per thread, long gaps): a
    /// colliding requester often fails the CAS but finds the lock released
    /// by the time it holds the spinlock. Both sides of the race must occur,
    /// and no request may be stranded; with clamp 1 co-pq rekeys at nearly
    /// every handoff.
    #[test]
    fn no_lost_wakeup_across_unlock_enqueue_races() {
        for step in [StepAside::Remote, StepAside::None] {
            let o = opts(step, None, 1);
            let dense = race::<FifoList>(o, 4, 3, 20_000, 2_000);
            let sparse = race::<FifoList>(o, 4, 1, 20_000, 100_000);
            assert!(
                dense.handoff > 0 && sparse.handoff > 0,
                "{dense:?} {sparse:?}"
            );
            assert!(sparse.free > 0, "{sparse:?}");
            let dense = race::<UsageList>(o, 4, 3, 20_000, 2_000);
            let sparse = race::<UsageList>(o, 4, 1, 20_000, 100_000);
            assert!(
                dense.handoff > 0 && sparse.handoff > 0,
                "{dense:?} {sparse:?}"
            );
            assert!(sparse.free > 0, "{sparse:?}");
            if step == StepAside::Remote {
                assert!(dense.step_asides > 0, "{dense:?}");
            } else {
                assert_eq!(dense.step_asides, 0, "{dense:?}");
            }
        }
    }

    /// Two cost classes (8:1) on 4 OS threads under pure usage order (clamp
    /// off): cheap handles get more requests served and the charged usage
    /// evens out; FIFO would give equal request counts.
    #[test]
    fn co_pq_serves_cheaper_handles_more() {
        const LIGHT: u64 = 2_000;
        const HEAVY: u64 = 8 * LIGHT;
        let lock = Arc::new(CoPq::with_options(0u64, opts(StepAside::Remote, None, 0)));
        let stop = Arc::new(AtomicBool::new(false));
        let served = Arc::new(AtomicU64::new(0));
        let out = Arc::new(Mutex::new(Vec::new()));
        let tasks: Vec<Vec<BoxFut>> = (0..4)
            .map(|_| {
                [LIGHT, HEAVY]
                    .into_iter()
                    .map(|cs| {
                        let (lock, stop, served, out) =
                            (lock.clone(), stop.clone(), served.clone(), out.clone());
                        Box::pin(async move {
                            let h = lock.handle();
                            let mut ops = 0u64;
                            while !stop.load(Relaxed) {
                                let g = h.lock().await;
                                spin_cycles(cs);
                                g.unlock().await;
                                ops += 1;
                                if served.fetch_add(1, Relaxed) + 1 >= 8_000 {
                                    stop.store(true, Relaxed);
                                }
                            }
                            out.lock().push((cs, ops, AsyncMutex::usage(&h)));
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

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Release {
        /// `g.unlock().await`.
        Unlock,
        /// `drop(g)`.
        DropGuard,
        /// `drop(g.unlock())` without polling it.
        DropUnlock,
    }

    struct Stress {
        /// Critical sections await `yield_now` while holding the guard with
        /// probability 1/n per op (xorshift; 0 = never).
        await_one_in: u64,
        release: Release,
        workers: usize,
        clients: usize,
        ops: u64,
        cs: u64,
        max_gap: u64,
    }

    struct Outcome {
        count: u64,
        overlaps: u64,
        stats: HandleStats,
        /// Longest inline chain seen inside a critical section.
        max_chain: u32,
        min_usage: u64,
    }

    /// `clients` tasks x `ops` non-atomic read-modify-writes on a real
    /// executor (balance 31), client i spawned on worker i mod `workers`.
    fn stress<Q: WaitList>(o: CoOptions, s: Stress) -> Outcome {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let lock = Arc::new(CoMutex::<Guarded, Q>::with_options(Guarded::default(), o));
            let exec = Executor::with_balance_interval(s.workers, 31);
            let tasks: Vec<_> = (0..s.clients)
                .map(|i| {
                    let lock = Arc::clone(&lock);
                    let (await_one_in, release, ops, cs, max_gap) =
                        (s.await_one_in, s.release, s.ops, s.cs, s.max_gap);
                    exec.spawn_on(i % s.workers, TaskKind::Client, async move {
                        let h = lock.handle();
                        let mut rng = (i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                        let mut last = 0;
                        let mut max_chain = 0;
                        for _ in 0..ops {
                            let mut g = h.lock().await;
                            max_chain = max_chain.max(executor::current_chain().0);
                            if g.inside {
                                g.overlaps += 1;
                            }
                            g.inside = true;
                            let before = g.count;
                            if await_one_in != 0 && rng % await_one_in == 0 {
                                // Unrelated await while owning the lock.
                                yield_now().await;
                            }
                            spin_cycles(cs);
                            g.count = before + 1;
                            g.inside = false;
                            let seen = g.count;
                            match release {
                                Release::Unlock => g.unlock().await,
                                Release::DropGuard => drop(g),
                                Release::DropUnlock => drop(g.unlock()),
                            }
                            assert!(seen > last, "count {seen} not after {last}");
                            last = seen;
                            rng ^= rng << 13;
                            rng ^= rng >> 7;
                            rng ^= rng << 17;
                            spin_cycles(rng % (max_gap + 1));
                        }
                        (h.stats(), max_chain, AsyncMutex::usage(&h))
                    })
                })
                .collect();
            let per = exec.block_on(async {
                let mut v = Vec::with_capacity(tasks.len());
                for t in tasks {
                    v.push(t.await);
                }
                v
            });
            let read = exec.spawn(TaskKind::Client, {
                let lock = Arc::clone(&lock);
                async move {
                    let h = lock.handle();
                    let g = h.lock().await;
                    let r = (g.count, g.overlaps);
                    g.unlock().await;
                    r
                }
            });
            let (count, overlaps) = exec.block_on(read);
            exec.shutdown();
            let mut stats = HandleStats::default();
            for (st, _, _) in &per {
                stats.add(st);
            }
            let _ = tx.send(Outcome {
                count,
                overlaps,
                stats,
                max_chain: per.iter().map(|p| p.1).max().unwrap_or(0),
                min_usage: per.iter().map(|p| p.2).min().unwrap_or(0),
            });
        });
        match rx.recv_timeout(DEADLINE) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => {
                panic!("run did not finish within {DEADLINE:?}: lost handoff or wake-up")
            }
            Err(RecvTimeoutError::Disconnected) => panic!("run panicked (see above)"),
        }
    }

    fn check<Q: WaitList>(o: CoOptions, s: Stress) -> Outcome {
        let name = format!(
            "{} {:?} k={:?} release={:?} await_one_in={} gap={}",
            Q::NAME,
            o.step_aside,
            o.chain_bound,
            s.release,
            s.await_one_in,
            s.max_gap
        );
        let (clients, ops, cs) = (s.clients as u64, s.ops, s.cs);
        let r = stress::<Q>(o, s);
        assert_eq!(r.overlaps, 0, "{name}: overlapping critical sections");
        assert_eq!(r.count, clients * ops, "{name}: lost or duplicated request");
        assert_eq!(
            r.stats.fast + r.stats.free + r.stats.handoff,
            clients * ops,
            "{name}"
        );
        assert!(
            r.min_usage >= ops * cs,
            "{name}: usage {} below the spin floor",
            r.min_usage
        );
        r
    }

    /// Every step-aside x chain bound {3, unbounded} x both policies;
    /// sustained (no gaps) and sparse (gaps up to 40 k cycles, so fast, free
    /// and handoff acquisitions mix).
    #[test]
    fn mutual_exclusion_and_completion_on_the_executor() {
        for step in STEPS {
            for k in [Some(3), None] {
                for (clients, ops, max_gap) in [(16, 2_000, 0), (8, 1_500, 40_000)] {
                    let s = || Stress {
                        await_one_in: 0,
                        release: Release::Unlock,
                        workers: 4,
                        clients,
                        ops,
                        cs: 200,
                        max_gap,
                    };
                    let o = opts(step, k, 1);
                    let f = check::<FifoList>(o, s());
                    let p = check::<UsageList>(o, s());
                    for r in [&f, &p] {
                        assert_eq!(r.stats.sync_drops, 0);
                        if step == StepAside::None {
                            assert_eq!(r.stats.step_asides, 0);
                        }
                    }
                    if max_gap == 0 && step != StepAside::None && k.is_none() {
                        assert!(f.stats.step_asides > 0 && p.stats.step_asides > 0);
                    }
                }
            }
        }
    }

    /// The owner awaits an unrelated future (`yield_now`, which lets every
    /// other task on its worker run) while holding the guard, in one op of
    /// three: it stays owner, the others queue behind it, nothing overlaps.
    #[test]
    fn guard_held_across_an_unrelated_await() {
        for step in STEPS {
            let s = || Stress {
                await_one_in: 3,
                release: Release::Unlock,
                workers: 4,
                clients: 16,
                ops: 1_500,
                cs: 200,
                max_gap: 0,
            };
            let o = opts(step, Some(DEFAULT_CHAIN_BOUND), 1);
            for r in [check::<FifoList>(o, s()), check::<UsageList>(o, s())] {
                assert!(
                    r.stats.handoff > 0,
                    "nobody queued behind an awaiting owner"
                );
            }
        }
    }

    /// Guards dropped without `unlock()`, and `unlock()` futures dropped
    /// unpolled: correct synchronous releases, all counted.
    #[test]
    fn sync_drop_release_is_correct() {
        for release in [Release::DropGuard, Release::DropUnlock] {
            for (clients, ops, max_gap) in [(16, 2_000, 0), (8, 1_500, 40_000)] {
                let s = || Stress {
                    await_one_in: 0,
                    release,
                    workers: 4,
                    clients,
                    ops,
                    cs: 200,
                    max_gap,
                };
                let o = opts(StepAside::Remote, Some(DEFAULT_CHAIN_BOUND), 1);
                for r in [check::<FifoList>(o, s()), check::<UsageList>(o, s())] {
                    assert_eq!(r.stats.sync_drops, clients as u64 * ops);
                    assert_eq!(r.stats.step_asides, 0);
                }
            }
        }
    }

    /// With K = 3 no critical section runs deeper than 3 inline resumes into
    /// a chain, the bound is reached, and chains are broken; unbounded, the
    /// same workload runs deeper chains (so the bound is what limits them).
    /// One and four workers. One critical section in 8 awaits while holding
    /// the guard: on one worker that is the only way requests can queue.
    #[test]
    fn chain_bound_limits_inline_chains() {
        const K: u32 = 3;
        for workers in [1, 4] {
            for step in STEPS {
                let s = || Stress {
                    await_one_in: 8,
                    release: Release::Unlock,
                    workers,
                    clients: 16,
                    ops: 1_000,
                    cs: 200,
                    max_gap: 0,
                };
                let r = check::<FifoList>(opts(step, Some(K), 1), s());
                assert_eq!(r.max_chain, K, "W={workers} {step:?}");
                assert!(r.stats.chain_breaks > 0, "W={workers} {step:?}");
                let r = check::<FifoList>(opts(step, None, 1), s());
                assert!(
                    r.max_chain > K,
                    "W={workers} {step:?}: max chain {}",
                    r.max_chain
                );
                assert_eq!(r.stats.chain_breaks, 0);
                for bound in [Some(K), None] {
                    let r = check::<UsageList>(opts(step, bound, 1), s());
                    assert_eq!(r.max_chain, 0, "co-pq must not resume inline");
                    assert_eq!(r.stats.chain_breaks, 0);
                }
            }
        }
    }
}
