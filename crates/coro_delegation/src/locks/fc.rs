//! Flat combining with FIFO service order, plus the delegation core that
//! [`super::fc_pq`] reuses with a usage-ordered policy.
//!
//! # Request protocol
//!
//! Every client owns one [`Node`], allocated once by [`DelegationLock::client`]
//! and kept alive by the lock (`Core::all`), so a combiner can never touch a
//! freed node even if the owning client is dropped right after its result
//! lands. `LockClient::run` returns a [`Run`] future that stores the closure
//! and the result slot inline. On its first poll the future
//!
//! 1. registers its `Waker` in the node,
//! 2. writes a raw pointer to its slot plus a type-erased trampoline into the
//!    node, marks the node `PENDING`,
//! 3. pushes the node onto the lock's publication stack (`Core::head`), and
//! 4. runs the combiner election: `try_lock` on `Core::combiner`.
//!
//! The election winner drains the stack in publication order into the
//! [`Policy`] queue and runs up to `FcOptions::pass_limit` closures on its
//! current executor worker, charging `cycles()` around each closure, marking
//! the request `COMPLETE` and waking the owner (`FcOptions::wake_placement`
//! decides where that wake lands). Losers return `Pending`; a waiter never
//! spins. A re-poll (spurious wake, or the task migrated and carries a new
//! `Waker`) re-registers the waker, re-checks completion, and runs the
//! election again.
//!
//! # Pass end and liveness
//!
//! Waiters are asleep, so the combiner may release the flag only when it has
//! ensured someone else will serve what is left:
//!
//! - queue empty: release, then re-read the publication stack (`SeqCst` on
//!   both sides, so a request that raced the release cannot be stranded) and
//!   re-elect if something arrived;
//! - work left and the combiner's own request is served (or the policy asks
//!   for rotation): release, then wake the policy's candidate (FIFO front /
//!   heap minimum / highest usage) *without* running its closure; the
//!   candidate re-polls, finds its request pending and runs the election;
//! - work left and own request pending: release and immediately re-elect,
//!   i.e. keep combining. This mirrors `libdlock` FC/FC-PQ, where a combiner
//!   loops until its own request completes.
//!
//! Every poll registers the waker *before* the election, so a candidate that
//! lost an earlier election is re-polled by the designation wake, which is
//! issued only after the flag is released.
//!
//! # Cooperative yield (`yield_after_combine`, default on)
//!
//! A poll that wins the election serves its own request in its pass, so
//! `run` would complete synchronously and the task would never return
//! `Pending`. On a cooperative executor without preemption or a coop budget
//! that task monopolises its worker while every waiter it just woke sits in
//! that worker's queue: with executor balancing off, one client did every
//! operation (service Jain 1/64). With `yield_after_combine` the poll instead
//! records the result, wakes itself with the plain `Waker` (default
//! placement, i.e. behind the waiters it woke) and returns `Pending` once;
//! the next poll returns `Ready` without touching the lock. The count is
//! exposed as `Client::combiner_yields`. OS-thread flat combining relies on
//! preemption for the same effect; `false` reproduces the greedy behaviour.
//!
//! # Cancellation
//!
//! Dropping a `Run` future after publication and before completion aborts the
//! process: the combiner may still dereference the slot. The harness never
//! cancels; this is a guard, not a feature. A critical section MUST NOT
//! panic: an unwinding closure leaves the combiner flag set forever.

use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::future::Future;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::ptr::{self, NonNull};
use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release, SeqCst};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicU8, AtomicUsize};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use crossbeam_utils::CachePadded;

use crate::executor::{wake_home, wake_remote, worker_id};
use crate::lock::{cycles, DelegationLock, LockClient};
use crate::stats::record_combining;

/// Default maximum number of closures one combining pass executes
/// (`FcOptions::pass_limit`).
pub const DEFAULT_PASS_LIMIT: usize = 64;

/// Where a wake issued by the lock lands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WakePlacement {
    /// Plain `Waker::wake_by_ref`: the executor's default (local queue of the
    /// waking worker).
    #[default]
    Default,
    /// `executor::wake_remote`: injector, so another worker may pick it up.
    Remote,
    /// `executor::wake_home`: the task's home worker.
    Home,
}

#[inline]
fn wake_with(waker: &Waker, placement: WakePlacement) {
    match placement {
        WakePlacement::Default => waker.wake_by_ref(),
        WakePlacement::Remote => wake_remote(waker),
        WakePlacement::Home => wake_home(waker),
    }
}

/// Executor-facing knobs shared by `Fc` and `FcPq`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FcOptions {
    /// See the module docs, "Cooperative yield". Default `true`.
    pub yield_after_combine: bool,
    /// Placement of every wake the lock issues: served waiters, the
    /// designated next combiner, and the cooperative self-yield.
    pub wake_placement: WakePlacement,
    /// Maximum closures per pass (`H`). Default [`DEFAULT_PASS_LIMIT`].
    pub pass_limit: usize,
}

impl Default for FcOptions {
    fn default() -> Self {
        Self {
            yield_after_combine: true,
            wake_placement: WakePlacement::Default,
            pass_limit: DEFAULT_PASS_LIMIT,
        }
    }
}

/// Request published, result not yet written.
const PENDING: u8 = 0;
/// Result written (or no request outstanding).
const COMPLETE: u8 = 1;

// ---------------------------------------------------------------------------
// Waker slot
// ---------------------------------------------------------------------------

const WAITING: usize = 0;
const REGISTERING: usize = 0b01;
const WAKING: usize = 0b10;

/// Single-owner waker slot with the register-then-recheck protocol of
/// `futures::task::AtomicWaker`: `register` is called only by the owning task
/// from `poll`, `wake` by any thread; a wake that races a registration is
/// delivered by the registrant, never lost.
pub struct AtomicWaker {
    state: AtomicUsize,
    waker: UnsafeCell<Option<Waker>>,
}

// SAFETY: the slot is accessed only under the REGISTERING / WAKING exclusion
// bits; `Waker` itself is `Send + Sync`.
unsafe impl Send for AtomicWaker {}
unsafe impl Sync for AtomicWaker {}

impl AtomicWaker {
    pub(crate) const fn new() -> Self {
        Self {
            state: AtomicUsize::new(WAITING),
            waker: UnsafeCell::new(None),
        }
    }

    /// Owner only. Stores `waker` (no clone when it would wake the same
    /// task). If a `wake` lands while we hold the slot, deliver it ourselves so
    /// the owner is re-polled and re-checks its condition.
    pub(crate) fn register(&self, waker: &Waker, placement: WakePlacement) {
        match self
            .state
            .compare_exchange(WAITING, REGISTERING, Acquire, Acquire)
        {
            Ok(_) => {
                // SAFETY: REGISTERING excludes `wake` from the slot.
                let slot = unsafe { &mut *self.waker.get() };
                match slot {
                    Some(old) if old.will_wake(waker) => {}
                    _ => *slot = Some(waker.clone()),
                }
                // Release the slot; if WAKING was set meanwhile the waker
                // must fire now (the waker thread saw REGISTERING and left it
                // to us). AcqRel: publishes the stored waker and acquires the
                // waker's `fetch_or`, i.e. the event it announces.
                if let Err(actual) =
                    self.state
                        .compare_exchange(REGISTERING, WAITING, AcqRel, Acquire)
                {
                    debug_assert_eq!(actual, REGISTERING | WAKING);
                    let pending = slot.take();
                    self.state.swap(WAITING, AcqRel);
                    if let Some(w) = pending {
                        wake_with(&w, placement);
                    }
                }
            }
            // A wake is in flight with the *old* waker; wake the new one too
            // so the owner re-polls with it registered.
            Err(WAKING) => wake_with(waker, placement),
            Err(_) => unreachable!("AtomicWaker::register: concurrent registration"),
        }
    }

    /// Any thread. Must be called after the event it announces is visible
    /// (here: after the `COMPLETE` release-store, or after the combiner flag
    /// is released for a designation wake).
    pub(crate) fn wake(&self, placement: WakePlacement) {
        // AcqRel: the Release half orders the caller's event before the
        // registrant's Acquire, the Acquire half orders the waker load after
        // its publication.
        if self.state.fetch_or(WAKING, AcqRel) == WAITING {
            // SAFETY: WAKING excludes `register` until we clear it.
            let pending = unsafe { (*self.waker.get()).take() };
            self.state.fetch_and(!WAKING, Release);
            if let Some(w) = pending {
                wake_with(&w, placement);
            }
        }
        // else: a registration is in progress and will deliver the wake.
    }
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

/// Slot pointer plus type-erased trampoline for one published request.
struct Request<T> {
    slot: *mut (),
    run: unsafe fn(*mut (), &mut T),
}

// Manual impls: `derive` would demand `T: Copy`.
impl<T> Clone for Request<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Request<T> {}

unsafe fn no_request<T>(_: *mut (), _: &mut T) {
    unreachable!("combiner ran a node without a published request")
}

/// Per-client delegation node. Allocated once per client, owned by the lock
/// (`Core::all`), cache-line aligned so a combiner's completion store does
/// not false-share with another client's node.
#[repr(align(128))]
pub struct Node<T> {
    /// `PENDING` from publication until the result is written; then
    /// `COMPLETE` (Release by the combiner, Acquire by the owner).
    state: AtomicU8,
    /// Publication-stack link. Written by the owner before the publishing
    /// CAS; owned by the combiner after the draining swap.
    next: AtomicPtr<Node<T>>,
    /// Written by the owner before the publishing CAS (Relaxed), read by the
    /// combiner after the SeqCst swap that took the node: the CAS/swap pair
    /// is the happens-before edge.
    req: UnsafeCell<Request<T>>,
    waker: AtomicWaker,
    /// Cumulative critical-section cycles charged by the policy. Written by
    /// the combiner, read by the owner (`usage()`); Relaxed is enough for an
    /// informational counter and, for FC-PQ, the ordering key is re-read by
    /// the combiner itself under the combiner flag.
    usage: AtomicU64,
    /// Requests served so far. Combiner-written; distinguishes a newcomer
    /// from a client whose usage was credited back to zero.
    served: AtomicU64,
    /// Cycles this client's task spent combining for others. Written and read
    /// only by the owner's task (it is the combiner when it writes).
    combining: AtomicU64,
    /// Cooperative yields after combining (`Core::yield_after_combine`).
    /// Owner-only, like `combining`.
    yields: AtomicU64,
    /// Link in `Core::all`, the ownership list; written once in `client()`
    /// before the node is reachable, read only by `Core::drop`.
    all_next: AtomicPtr<Node<T>>,
}

// SAFETY: the raw pointers inside are dereferenced only under the protocol
// above (owner before publication / after completion, combiner in between).
unsafe impl<T> Send for Node<T> {}
unsafe impl<T> Sync for Node<T> {}

impl<T> Node<T> {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(COMPLETE),
            next: AtomicPtr::new(ptr::null_mut()),
            all_next: AtomicPtr::new(ptr::null_mut()),
            req: UnsafeCell::new(Request {
                slot: ptr::null_mut(),
                run: no_request::<T>,
            }),
            waker: AtomicWaker::new(),
            usage: AtomicU64::new(0),
            served: AtomicU64::new(0),
            combining: AtomicU64::new(0),
            yields: AtomicU64::new(0),
        }
    }

    /// Combiner side. Cumulative charged cycles.
    pub fn usage(&self) -> u64 {
        self.usage.load(Relaxed)
    }

    /// Combiner side. Overwrites the charged cycles and counts the service.
    pub fn charge(&self, usage: u64) {
        self.usage.store(usage, Relaxed);
        self.served.store(self.served.load(Relaxed) + 1, Relaxed);
    }

    /// Combiner side. Overwrites the charged cycles without counting a
    /// service (credits).
    pub fn set_usage(&self, usage: u64) {
        self.usage.store(usage, Relaxed);
    }

    /// Combiner side. Requests served so far.
    pub fn served(&self) -> u64 {
        self.served.load(Relaxed)
    }

    /// True while a published request has not been served. Only meaningful
    /// to the combiner (which is the only writer of `COMPLETE`) or the owner.
    fn is_pending(&self) -> bool {
        self.state.load(Relaxed) == PENDING
    }
}

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

/// Service-order policy. Every method runs on the combiner, under the
/// combiner flag; the policy is the only holder of admitted nodes until it
/// returns them from `next`.
pub trait Policy<T>: Send + 'static {
    const NAME: &'static str;

    /// Admit a freshly published node. Called in publication order.
    fn admit(&mut self, node: NonNull<Node<T>>, pass: u64);

    /// Called once per pass after admission, before the first `next`.
    fn begin_pass(&mut self, _pass: u64) {}

    /// Next request to serve; the policy remembers it for `charge`.
    fn next(&mut self) -> Option<NonNull<Node<T>>>;

    /// Charge `cs` cycles to the request last returned by `next`.
    fn charge(&mut self, cs: u64);

    fn is_empty(&self) -> bool;

    /// Node to hand the combiner role to (left in the queue).
    fn candidate(&self) -> Option<NonNull<Node<T>>>;

    /// Stop the pass once this many cycles have elapsed, in addition to `H`.
    fn pass_budget(&self) -> Option<u64> {
        None
    }

    /// Hand off at every pass end even if the combiner's own request is
    /// still queued.
    fn rotate(&self) -> bool {
        false
    }

    /// Credit `cycles` of combining work to `node` (`credit_combining`).
    fn credit(&mut self, _node: NonNull<Node<T>>, _cycles: u64) {}
}

// ---------------------------------------------------------------------------
// Core
// ---------------------------------------------------------------------------

/// Combiner-private state; every access is under `Core::combiner`.
struct Inner<T, P> {
    data: T,
    policy: P,
    pass: u64,
}

/// Delegation core: publication stack, combiner flag, protected data and the
/// policy queue. `Fc<T>` and `FcPq<T>` are instantiations.
pub struct Core<T, P> {
    combiner: CachePadded<AtomicBool>,
    head: CachePadded<AtomicPtr<Node<T>>>,
    inner: UnsafeCell<Inner<T, P>>,
    /// Intrusive list (via `Node::all_next`) of every node ever handed to a
    /// client. Nodes are freed only in `Drop`, and every client keeps the
    /// lock alive through its `Arc`, so a combiner's post-completion `wake`
    /// never dangles even if the owning client is dropped meanwhile.
    all: AtomicPtr<Node<T>>,
    opts: FcOptions,
}

// SAFETY: `inner` is accessed only by the combiner-flag holder; `all` is an
// atomic list; `T: Send` suffices because `T` is only ever touched by one
// thread at a time (the combiner).
unsafe impl<T: Send, P: Send> Send for Core<T, P> {}
unsafe impl<T: Send, P: Send> Sync for Core<T, P> {}

impl<T, P> Core<T, P> {
    /// Free every node in `all`. Requires `&mut self`: the last `Arc` is
    /// gone, so no client and no combiner is alive.
    fn free_nodes(&mut self) {
        let mut p = std::mem::replace(self.all.get_mut(), ptr::null_mut());
        while !p.is_null() {
            // SAFETY: every pointer in `all` came from `Box::into_raw` in
            // `client()` and is freed exactly once here.
            let node = unsafe { Box::from_raw(p) };
            p = node.all_next.load(Relaxed);
        }
    }

    /// Take the protected value back once every client is gone.
    pub fn into_inner(self) -> T {
        let mut this = std::mem::ManuallyDrop::new(self);
        this.free_nodes();
        // SAFETY: `this` is never dropped or used again; the remaining
        // fields are atomics without destructors.
        unsafe { ptr::read(this.inner.get()) }.data
    }
}

impl<T, P> Drop for Core<T, P> {
    fn drop(&mut self) {
        self.free_nodes();
    }
}

impl<T: Send + 'static, P: Policy<T>> Core<T, P> {
    pub fn with_policy(data: T, policy: P, opts: FcOptions) -> Self {
        Self {
            combiner: CachePadded::new(AtomicBool::new(false)),
            head: CachePadded::new(AtomicPtr::new(ptr::null_mut())),
            inner: UnsafeCell::new(Inner {
                data,
                policy,
                pass: 0,
            }),
            all: AtomicPtr::new(ptr::null_mut()),
            opts,
        }
    }

    /// The executor-facing knobs this lock was built with.
    pub fn options(&self) -> FcOptions {
        self.opts
    }

    #[inline]
    fn try_lock(&self) -> bool {
        // SeqCst on the flag and on `head` (publish CAS, drain swap, the
        // post-release load) puts the four Dekker-style accesses in one total
        // order: either the releasing combiner sees the newcomer's node, or
        // the newcomer sees the released flag.
        self.combiner
            .compare_exchange(false, true, SeqCst, SeqCst)
            .is_ok()
    }

    #[inline]
    fn unlock(&self) {
        self.combiner.store(false, SeqCst);
    }

    /// Push onto the publication stack. Success is SeqCst: it releases the
    /// node's `req`/`state`/`next` writes to the draining combiner and takes
    /// part in the Dekker order with the flag.
    fn publish(&self, node: &Node<T>) {
        let node_ptr = node as *const Node<T> as *mut Node<T>;
        let mut head = self.head.load(Relaxed);
        loop {
            node.next.store(head, Relaxed);
            match self
                .head
                .compare_exchange_weak(head, node_ptr, SeqCst, Relaxed)
            {
                Ok(_) => return,
                Err(h) => head = h,
            }
        }
    }

    /// Combiner only. Take the whole stack and admit it in publication order.
    fn drain(&self, inner: &mut Inner<T, P>) {
        let mut p = self.head.swap(ptr::null_mut(), SeqCst);
        if p.is_null() {
            return;
        }
        // Reverse the LIFO chain in place; the swap made every link ours.
        let mut prev: *mut Node<T> = ptr::null_mut();
        while !p.is_null() {
            // SAFETY: nodes live as long as the lock; the pusher's `next`
            // store happens-before our SeqCst swap of `head`.
            let n = unsafe { &*p };
            let nx = n.next.load(Relaxed);
            n.next.store(prev, Relaxed);
            prev = p;
            p = nx;
        }
        let mut p = prev;
        while !p.is_null() {
            // SAFETY: as above.
            let n = unsafe { &*p };
            let nx = n.next.load(Relaxed);
            // SAFETY: `p` is non-null (loop condition).
            inner
                .policy
                .admit(unsafe { NonNull::new_unchecked(p) }, inner.pass);
            p = nx;
        }
    }

    /// Run combining passes. Entered with the combiner flag held; returns
    /// with it released and the liveness obligations of the module docs met.
    fn combine(&self, me: &Node<T>) {
        // SAFETY: we hold the combiner flag, the only path to `inner`.
        let inner = unsafe { &mut *self.inner.get() };
        let worker = worker_id();
        loop {
            let pass_begin = cycles();
            inner.pass += 1;
            let pass = inner.pass;
            self.drain(inner);
            inner.policy.begin_pass(pass);
            let budget = inner.policy.pass_budget();

            let mut own_cs = 0u64;
            for _ in 0..self.opts.pass_limit {
                let Some(node_ptr) = inner.policy.next() else {
                    break;
                };
                // SAFETY: the node is published (PENDING) and lives as long
                // as the lock; the owner's future is pinned and undropped
                // while PENDING (Drop aborts otherwise), so `req.slot` is
                // valid. Only we run the closure: the policy handed the node
                // out exactly once.
                let node = unsafe { node_ptr.as_ref() };
                let req = unsafe { *node.req.get() };
                let begin = cycles();
                unsafe { (req.run)(req.slot, &mut inner.data) };
                let end = cycles();
                let cs = end - begin;
                inner.policy.charge(cs);
                // Release: publishes the result written by the trampoline
                // and the usage store to the owner's Acquire load.
                node.state.store(COMPLETE, Release);
                if ptr::eq(node, me) {
                    own_cs += cs;
                } else {
                    node.waker.wake(self.opts.wake_placement);
                }
                if let Some(b) = budget {
                    if end - pass_begin >= b {
                        break;
                    }
                }
            }

            // Arrivals during the pass must be visible to the hand-off
            // decision below.
            self.drain(inner);

            let pass_cycles = cycles() - pass_begin;
            let admin = pass_cycles - own_cs;
            me.combining
                .store(me.combining.load(Relaxed) + admin, Relaxed);
            if let Some(w) = worker {
                record_combining(w, pass_cycles);
            }
            inner.policy.credit(NonNull::from(me), admin);

            if inner.policy.is_empty() {
                self.unlock();
                // Dekker re-check (SeqCst pairs with `publish`): a newcomer
                // that pushed before our release and lost its election must
                // not be stranded.
                if self.head.load(SeqCst).is_null() || !self.try_lock() {
                    return;
                }
                continue;
            }

            let own_done = !me.is_pending();
            if own_done || inner.policy.rotate() {
                let cand = inner
                    .policy
                    .candidate()
                    .expect("non-empty policy has a candidate");
                if cand != NonNull::from(me) {
                    // Release first, then wake: the candidate's poll must be
                    // able to win the flag.
                    self.unlock();
                    // SAFETY: nodes live as long as the lock.
                    unsafe { cand.as_ref() }
                        .waker
                        .wake(self.opts.wake_placement);
                    return;
                }
                // The candidate is us: nothing to hand off to; keep going.
            }

            // Own request still queued: re-elect immediately (the reference
            // combiner loops until its request completes). Losing the race
            // is fine: the winner owns the queue.
            self.unlock();
            if !self.try_lock() {
                return;
            }
        }
    }
}

impl<T: Send + 'static, P: Policy<T> + Default> DelegationLock<T> for Core<T, P> {
    type Client = Client<T, P>;

    fn new(data: T) -> Self {
        Self::with_policy(data, P::default(), FcOptions::default())
    }

    fn client(self: &Arc<Self>) -> Client<T, P> {
        let node = Box::into_raw(Box::new(Node::new()));
        let mut head = self.all.load(Relaxed);
        loop {
            // SAFETY: `node` is ours until the CAS publishes it to `all`,
            // whose only reader is `Drop`.
            unsafe { (*node).all_next.store(head, Relaxed) };
            match self.all.compare_exchange_weak(head, node, Release, Relaxed) {
                Ok(_) => break,
                Err(h) => head = h,
            }
        }
        Client {
            core: Arc::clone(self),
            // SAFETY: `Box::into_raw` never returns null.
            node: unsafe { NonNull::new_unchecked(node) },
        }
    }

    fn name() -> &'static str {
        P::NAME
    }
}

// ---------------------------------------------------------------------------
// Client and the `run` future
// ---------------------------------------------------------------------------

pub struct Client<T, P> {
    core: Arc<Core<T, P>>,
    /// Owned by `core.all`; valid while `core` is alive.
    node: NonNull<Node<T>>,
}

// SAFETY: `node` is only dereferenced by the owning task (or by the combiner
// through the publication protocol); `Node` is `Sync`.
unsafe impl<T: Send, P: Send> Send for Client<T, P> {}

impl<T: Send + 'static, P: Policy<T>> Client<T, P> {
    #[inline]
    fn node(&self) -> &Node<T> {
        // SAFETY: `core` (held by `self`) owns the node.
        unsafe { self.node.as_ref() }
    }

    /// Test only: the client's node, to read its counters after the client
    /// has been moved into a task.
    #[cfg(test)]
    pub(crate) fn node_ptr(&self) -> NonNull<Node<T>> {
        self.node
    }
}

#[cfg(test)]
impl<T, P> Core<T, P> {
    /// Test only: the protected value. Caller guarantees no combiner is
    /// active (every client finished).
    pub(crate) fn data(&self) -> &T {
        // SAFETY: see above.
        unsafe { &(*self.inner.get()).data }
    }

    /// Test only: combining passes run so far. Caller guarantees no combiner
    /// is active.
    pub(crate) fn passes(&self) -> u64 {
        // SAFETY: see above.
        unsafe { (*self.inner.get()).pass }
    }

    /// Test only: take the combiner flag from outside a `run` poll so that
    /// later polls publish and lose the election.
    pub(crate) fn hold_combiner(&self) -> bool {
        self.combiner
            .compare_exchange(false, true, SeqCst, SeqCst)
            .is_ok()
    }

    /// Test only: release a flag taken by `hold_combiner`.
    pub(crate) fn release_combiner(&self) {
        self.combiner.store(false, SeqCst);
    }
}

impl<T: Send + 'static, P: Policy<T>> LockClient<T> for Client<T, P> {
    fn run<R, F>(&mut self, f: F) -> impl Future<Output = R> + Send
    where
        R: Send,
        F: FnOnce(&mut T) -> R + Send,
    {
        Run {
            client: self,
            slot: UnsafeCell::new(Slot::Pending(f)),
            stage: Stage::Unpublished,
            _pin: PhantomPinned,
        }
    }

    fn usage(&self) -> u64 {
        self.node().usage.load(Relaxed)
    }

    fn combining_cycles(&self) -> u64 {
        self.node().combining.load(Relaxed)
    }
}

impl<T: Send + 'static, P: Policy<T>> Client<T, P> {
    /// Number of `run` polls that combined, completed their own request and
    /// returned `Pending` once instead of `Ready` (`yield_after_combine`).
    pub fn combiner_yields(&self) -> u64 {
        self.node().yields.load(Relaxed)
    }
}

enum Slot<R, F> {
    Pending(F),
    Done(R),
    Empty,
}

/// Type-erased entry point the combiner calls: consumes the closure, stores
/// the result in place.
unsafe fn run_slot<T, R, F: FnOnce(&mut T) -> R>(slot: *mut (), data: &mut T) {
    // SAFETY: `slot` was produced from `&Run::slot` of a pinned, live future
    // whose node is PENDING; the combiner has exclusive access until it
    // stores COMPLETE.
    let slot = unsafe { &mut *slot.cast::<Slot<R, F>>() };
    let f = match std::mem::replace(slot, Slot::Empty) {
        Slot::Pending(f) => f,
        _ => unreachable!("request run twice"),
    };
    *slot = Slot::Done(f(data));
}

#[derive(PartialEq, Eq)]
enum Stage {
    Unpublished,
    Published,
    /// Own request complete after combining; returned `Pending` once.
    Yielded,
    Done,
}

/// Future returned by `LockClient::run`. Owns the closure and the result
/// inline; `!Unpin` because the combiner holds a raw pointer to `slot` while
/// `stage == Published`.
pub struct Run<'a, T, P, R, F> {
    client: &'a mut Client<T, P>,
    slot: UnsafeCell<Slot<R, F>>,
    stage: Stage,
    _pin: PhantomPinned,
}

impl<T, P, R, F> Future for Run<'_, T, P, R, F>
where
    T: Send + 'static,
    P: Policy<T>,
    F: FnOnce(&mut T) -> R,
{
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<R> {
        // SAFETY: we never move out of `this`; `slot`'s address is stable for
        // the future's lifetime because it is pinned.
        let this = unsafe { self.get_unchecked_mut() };
        let core = &*this.client.core;
        let node = this.client.node();

        match this.stage {
            Stage::Unpublished => {
                node.waker.register(cx.waker(), core.opts.wake_placement);
                // SAFETY: not yet published, so no combiner reads `req`.
                unsafe {
                    *node.req.get() = Request {
                        slot: this.slot.get().cast::<()>(),
                        run: run_slot::<T, R, F>,
                    };
                }
                node.state.store(PENDING, Relaxed);
                this.stage = Stage::Published;
                core.publish(node);
            }
            Stage::Published => {
                if node.state.load(Acquire) == COMPLETE {
                    return Poll::Ready(this.finish());
                }
                node.waker.register(cx.waker(), core.opts.wake_placement);
                // Re-check after registering: a completion that raced the
                // registration is either seen here or delivered as a wake.
                if node.state.load(Acquire) == COMPLETE {
                    return Poll::Ready(this.finish());
                }
            }
            // Combined last poll and yielded; the result is already ours.
            Stage::Yielded => return Poll::Ready(this.finish()),
            Stage::Done => panic!("`run` future polled after completion"),
        }

        // Election. The waker is registered, so if we lose and are later
        // designated as combiner the designation wake re-polls us.
        if core.try_lock() {
            core.combine(node);
            if node.state.load(Acquire) == COMPLETE {
                if core.opts.yield_after_combine {
                    // Cooperative yield (module docs): let the waiters this
                    // pass woke onto our worker run before we continue.
                    this.stage = Stage::Yielded;
                    node.yields.store(node.yields.load(Relaxed) + 1, Relaxed);
                    wake_with(cx.waker(), core.opts.wake_placement);
                    return Poll::Pending;
                }
                return Poll::Ready(this.finish());
            }
            return Poll::Pending;
        }
        if node.state.load(Acquire) == COMPLETE {
            return Poll::Ready(this.finish());
        }
        Poll::Pending
    }
}

impl<T, P, R, F> Run<'_, T, P, R, F> {
    /// Called after observing `COMPLETE` with Acquire.
    fn finish(&mut self) -> R {
        self.stage = Stage::Done;
        // SAFETY: the combiner released its access with the COMPLETE store.
        match std::mem::replace(unsafe { &mut *self.slot.get() }, Slot::Empty) {
            Slot::Done(r) => r,
            _ => unreachable!("completed request without result"),
        }
    }
}

impl<T, P, R, F> Drop for Run<'_, T, P, R, F> {
    fn drop(&mut self) {
        if self.stage == Stage::Published {
            // SAFETY: `client.node` is valid while `client.core` lives.
            let node = unsafe { self.client.node.as_ref() };
            if node.state.load(Acquire) != COMPLETE {
                // The combiner may still dereference our slot. Cancellation
                // after publication is outside the contract (lock.rs).
                eprintln!("coro_delegation: `run` future dropped while published; aborting");
                std::process::abort();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// FIFO policy = flat combining
// ---------------------------------------------------------------------------

/// FIFO over publication order.
pub struct Fifo<T> {
    queue: VecDeque<NonNull<Node<T>>>,
    current: Option<NonNull<Node<T>>>,
}

// SAFETY: the queued pointers are dereferenced only by the combiner.
unsafe impl<T> Send for Fifo<T> {}

impl<T> Default for Fifo<T> {
    fn default() -> Self {
        Self {
            // Bounded by the number of concurrently pending requests, i.e. by
            // the number of clients; sized so the steady state never grows it.
            queue: VecDeque::with_capacity(1024),
            current: None,
        }
    }
}

impl<T: Send + 'static> Policy<T> for Fifo<T> {
    const NAME: &'static str = "fc";

    fn admit(&mut self, node: NonNull<Node<T>>, _pass: u64) {
        self.queue.push_back(node);
    }

    fn next(&mut self) -> Option<NonNull<Node<T>>> {
        self.current = self.queue.pop_front();
        self.current
    }

    fn charge(&mut self, cs: u64) {
        // SAFETY: `current` came from `next` and is still PENDING.
        let node = unsafe { self.current.expect("charge without next").as_ref() };
        node.charge(node.usage() + cs);
    }

    fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    fn candidate(&self) -> Option<NonNull<Node<T>>> {
        self.queue.front().copied()
    }
}

/// Flat combining, FIFO service order.
pub type Fc<T> = Core<T, Fifo<T>>;

impl<T: Send + 'static> Fc<T> {
    /// `Fc` with explicit [`FcOptions`] (`DelegationLock::new` uses the
    /// defaults).
    pub fn with_options(data: T, opts: FcOptions) -> Self {
        Core::with_policy(data, Fifo::default(), opts)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod testing {
    //! Minimal executors for lock tests: real wakers (an `Arc<Flag>` per
    //! task) so lost wakeups surface as a "no runnable task" panic instead
    //! of being masked by round-robin polling.

    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering::{AcqRel, Release};
    use std::sync::Arc;
    use std::task::{Context, Wake, Waker};
    use std::thread::{self, Thread};
    use std::time::{Duration, Instant};

    use crate::lock::cycles;

    pub type BoxFut = Pin<Box<dyn Future<Output = ()> + Send>>;

    struct Flag {
        ready: AtomicBool,
        thread: Thread,
    }

    impl Wake for Flag {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.ready.store(true, Release);
            self.thread.unpark();
        }
    }

    pub fn spin_cycles(n: u64) {
        let t0 = cycles();
        while cycles() - t0 < n {
            std::hint::spin_loop();
        }
    }

    /// Drive `tasks` to completion on the calling thread, polling only tasks
    /// whose waker fired. Panics if nothing is runnable for `stall`.
    pub fn run_all(mut tasks: Vec<BoxFut>, stall: Duration) {
        let n = tasks.len();
        let flags: Vec<Arc<Flag>> = (0..n)
            .map(|_| {
                Arc::new(Flag {
                    ready: AtomicBool::new(true),
                    thread: thread::current(),
                })
            })
            .collect();
        let wakers: Vec<Waker> = flags.iter().map(|f| Waker::from(Arc::clone(f))).collect();
        let mut done = vec![false; n];
        let mut remaining = n;
        let mut idle_since: Option<Instant> = None;
        while remaining > 0 {
            let mut progressed = false;
            for i in 0..n {
                if done[i] || !flags[i].ready.swap(false, AcqRel) {
                    continue;
                }
                progressed = true;
                let mut cx = Context::from_waker(&wakers[i]);
                if tasks[i].as_mut().poll(&mut cx).is_ready() {
                    done[i] = true;
                    remaining -= 1;
                }
            }
            if progressed {
                idle_since = None;
                continue;
            }
            let since = *idle_since.get_or_insert_with(Instant::now);
            assert!(
                since.elapsed() < stall,
                "no runnable task for {stall:?} with {remaining} unfinished: lost wakeup"
            );
            thread::park_timeout(Duration::from_millis(1));
        }
    }

    /// `threads` executors, each driving its own slice of `tasks`.
    pub fn run_threads(tasks: Vec<Vec<BoxFut>>, stall: Duration) {
        thread::scope(|s| {
            for slice in tasks {
                s.spawn(move || run_all(slice, stall));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{run_all, run_threads, spin_cycles, BoxFut};
    use super::*;
    use std::time::Duration;

    const STALL: Duration = Duration::from_secs(5);

    fn counter_task(mut client: Client<u64, Fifo<u64>>, ops: usize, cs: u64) -> BoxFut {
        Box::pin(async move {
            for _ in 0..ops {
                client
                    .run(move |c| {
                        *c += 1;
                        spin_cycles(cs);
                    })
                    .await;
            }
        })
    }

    #[test]
    fn fifo_16_clients_1000_ops_single_thread() {
        let lock = Arc::new(Fc::<u64>::new(0));
        let clients: Vec<_> = (0..16).map(|_| lock.client()).collect();
        let tasks = clients
            .into_iter()
            .map(|c| counter_task(c, 1000, 200))
            .collect();
        run_all(tasks, STALL);
        // SAFETY: no client is alive; the flag is free.
        let count = *lock.data();
        assert_eq!(count, 16_000);
    }

    #[test]
    fn every_wake_placement_builds_and_counts() {
        for placement in [
            WakePlacement::Default,
            WakePlacement::Remote,
            WakePlacement::Home,
        ] {
            let lock = Arc::new(Fc::<u64>::with_options(
                0,
                FcOptions {
                    wake_placement: placement,
                    ..FcOptions::default()
                },
            ));
            assert_eq!(lock.options().wake_placement, placement);
            let tasks: Vec<BoxFut> = (0..8)
                .map(|_| counter_task(lock.client(), 200, 100))
                .collect();
            run_all(tasks, STALL);
            assert_eq!(*lock.data(), 1600, "{placement:?}");
        }
    }

    /// Service order under a cooperative single-thread executor whose tasks
    /// never yield on their own: with `yield_after_combine` every op hands
    /// the worker back, so the clients interleave; without it the first
    /// client runs all of its ops before anyone else is polled.
    fn service_order(yield_after_combine: bool) -> (Vec<usize>, Vec<u64>) {
        let clients = 4;
        let ops = 50;
        let lock = Arc::new(Fc::<Vec<usize>>::with_options(
            Vec::with_capacity(clients * ops),
            FcOptions {
                yield_after_combine,
                ..FcOptions::default()
            },
        ));
        let yields = Arc::new((0..clients).map(|_| AtomicU64::new(0)).collect::<Vec<_>>());
        let tasks = (0..clients)
            .map(|i| {
                let mut c = lock.client();
                let yields = Arc::clone(&yields);
                Box::pin(async move {
                    for _ in 0..ops {
                        c.run(move |log| log.push(i)).await;
                    }
                    yields[i].store(c.combiner_yields(), Relaxed);
                }) as BoxFut
            })
            .collect();
        run_all(tasks, STALL);
        let log = Arc::try_unwrap(lock)
            .ok()
            .expect("clients gone")
            .into_inner();
        assert_eq!(log.len(), clients * ops);
        (log, yields.iter().map(|y| y.load(Relaxed)).collect())
    }

    #[test]
    fn yield_after_combine_interleaves_greedy_tasks() {
        let (log, yields) = service_order(true);
        // Uncontended: every run combines itself, so every op yields once.
        assert_eq!(yields, vec![50, 50, 50, 50]);
        // Round-robin interleaving: the first 4 ops come from 4 clients.
        let mut first: Vec<usize> = log[..4].to_vec();
        first.sort_unstable();
        assert_eq!(first, vec![0, 1, 2, 3], "log head {:?}", &log[..8]);
    }

    #[test]
    fn no_yield_lets_the_combiner_monopolise_the_worker() {
        let (log, yields) = service_order(false);
        assert_eq!(yields, vec![0, 0, 0, 0]);
        assert!(
            log[..50].iter().all(|&c| c == 0),
            "log head {:?}",
            &log[..8]
        );
    }

    #[test]
    fn fifo_multi_thread_counts_and_accounts() {
        let lock = Arc::new(Fc::<u64>::new(0));
        let threads = 8;
        let per_thread = 8;
        let ops = 2000;
        let mut all_clients = Vec::new();
        let tasks: Vec<Vec<BoxFut>> = (0..threads)
            .map(|_| {
                (0..per_thread)
                    .map(|_| {
                        let c = lock.client();
                        // Keep a second handle-free view: usage is on the node.
                        all_clients.push(c.node);
                        counter_task(c, ops, 300)
                    })
                    .collect()
            })
            .collect();
        run_threads(tasks, STALL);
        let count = *lock.data();
        assert_eq!(count, (threads * per_thread * ops) as u64);
        let usage: Vec<u64> = all_clients
            .iter()
            .map(|n| unsafe { n.as_ref() }.usage())
            .collect();
        // Every client's charged cycles cover its own spin time.
        for u in &usage {
            assert!(*u >= ops as u64 * 300, "usage {u} below spin floor");
        }
        let combining: u64 = all_clients
            .iter()
            .map(|n| unsafe { n.as_ref() }.combining.load(Relaxed))
            .sum();
        assert!(combining > 0, "nobody combined for anyone else");
    }

    #[test]
    fn atomic_waker_register_wake_race_never_loses() {
        // Owner registers/re-registers while another thread wakes: every
        // wake must be delivered to *some* registration or `wake_by_ref`.
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        use std::task::Wake;
        struct Count(AtomicUsize);
        impl Wake for Count {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, SeqCst);
            }
        }
        let slot = Arc::new(AtomicWaker::new());
        let counter = Arc::new(Count(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&counter));
        let rounds = 200_000;
        let s2 = Arc::clone(&slot);
        let waker_thread = std::thread::spawn(move || {
            for _ in 0..rounds {
                s2.wake(WakePlacement::Default);
            }
        });
        for _ in 0..rounds {
            slot.register(&waker, WakePlacement::Default);
        }
        waker_thread.join().unwrap();
        slot.wake(WakePlacement::Default);
        // Either the wake found the waker (counted) or a register raced it
        // and delivered via `wake_by_ref` (also counted): at least the final
        // wake lands, and no panic from a bad state.
        assert!(counter.0.load(SeqCst) >= 1);
    }
}
