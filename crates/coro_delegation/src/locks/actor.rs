//! `actor` / `actor-inline`: the combiner is a dedicated **server task** per
//! lock (the async-runtime "actor" idiom applied to delegation).
//!
//! # Request protocol
//!
//! Clients publish exactly as in [`super::fc`]: one `Record` per client,
//! allocated by [`DelegationLock::client`] and owned by the lock
//! (`Actor::all`); `run` keeps the closure and the result slot inline in the
//! returned future, registers the task's waker, marks the record `PENDING`
//! and pushes it onto a Treiber stack (`Actor::head`). There is no election:
//! the client only makes sure the server will look at the stack (below) and
//! returns `Pending`. It completes when it observes `COMPLETE` (Acquire),
//! which the server stores (Release) after writing the result and adding the
//! closure's `cycles()` to the record's `usage`, i.e. the completion protocol
//! and the per-request accounting of `fc`.
//!
//! # Server
//!
//! One task per lock, spawned lazily on the executor by the first request
//! (`executor::spawn_here`, the only executor hook). Each poll:
//!
//! 1. drain the stack (one `swap`), reverse it in place and append it to the
//!    server-private intrusive FIFO;
//! 2. FIFO non-empty: run one *pass* of up to `pass_limit` closures in FIFO
//!    order, each timed with `cycles()`, storing `COMPLETE` and waking the
//!    owner with `wake_placement`; charge the pass (drain included) to the
//!    worker that ran it (`stats::record_combining`), so burden Jain and
//!    per-worker CS share come out exactly as for `ces`/`fc`; then yield
//!    (self-wake under `yield_placement`, return `Pending`);
//! 3. FIFO empty: store its waker, publish `IDLE`, re-check (below), return
//!    `Pending`.
//!
//! # Server state (`Actor::server`): `ABSENT` / `RUNNING` / `IDLE`
//!
//! Every access is `SeqCst`, as are the stack push and drain, so neither
//! Dekker pair can miss both ways:
//!
//! - publisher: push, then load the state. `IDLE`: CAS to `RUNNING` and wake
//!   the server (`server_wake` placement). `ABSENT`: CAS to `RUNNING` and
//!   spawn a server (same placement). `RUNNING`, or a lost CAS: nothing; the
//!   running server re-checks the stack before it can go idle.
//! - server: store `IDLE`, then load the stack head and the live-client
//!   count; if either calls for it, CAS `IDLE` -> `RUNNING` itself and carry
//!   on. A lost CAS means someone else won it and a wake is in flight.
//!
//! Whoever wins `IDLE` -> `RUNNING` owns the waker slot
//! (`Actor::server_waker`): the server fills it before storing `IDLE` and is
//! not polled again until the winner has taken the waker and woken it.
//!
//! # Lifetime
//!
//! The server exits (its task completes, dropping its `Arc` of the lock) when
//! it is idle and no client is alive (`Actor::live`); the last client's
//! `Drop` wakes an idle server for that, with the same pairing (decrement,
//! then load the state; against: store `IDLE`, then load the count). A later
//! request spawns a new server. Otherwise the idle server and the lock would
//! keep each other alive past executor shutdown. Exits need every client
//! gone, so the steady state allocates nothing: records come from `client()`,
//! the server task from one spawn.
//!
//! # Variants
//!
//! | knob | `actor` ([`ActorOptions::plain`]) | `actor-inline` ([`ActorOptions::inline`]) |
//! |---|---|---|
//! | `server_wake` (publisher wakes / spawns an idle server) | default: local queue of the publisher's worker | inline: run-next slot of the publisher's worker |
//! | `yield_placement` (after every pass) | default: back of the local queue | home: inbox of the worker that just polled it |
//! | `wake_placement` (client completions) | default | remote (as `fc-remote`) |
//!
//! # Cancellation and panics
//!
//! As `fc`: dropping a `run` future after publication and before completion
//! aborts (the server may still write its slot). A panicking critical
//! section kills the server task and every later request hangs.

use std::cell::UnsafeCell;
use std::future::Future;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::ptr::{self, NonNull};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release, SeqCst};
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicU8, AtomicUsize};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use crossbeam_utils::CachePadded;

use super::fc::{AtomicWaker, WakePlacement, DEFAULT_PASS_LIMIT};
use crate::executor::{self, Placement};
use crate::lock::{cycles, DelegationLock, LockClient};
use crate::stats::{record_combining, TaskKind};

/// Request published, result not yet written.
const PENDING: u8 = 0;
/// Result written (or no request outstanding).
const COMPLETE: u8 = 1;

/// No server task exists.
const ABSENT: u8 = 0;
/// A server task exists and will drain the stack before it can go idle.
const RUNNING: u8 = 1;
/// The server task is parked with its waker in `Actor::server_waker`.
const IDLE: u8 = 2;

/// Executor-facing knobs; see the module docs, "Variants".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActorOptions {
    /// Placement of the wake (or spawn) a publisher issues when it finds the
    /// server idle (or absent).
    pub server_wake: Placement,
    /// Placement of the server's self-reschedule after every pass. `Inline`
    /// would re-poll it at once, i.e. never yield; it is treated as
    /// `Default`.
    pub yield_placement: Placement,
    /// Placement of every client completion wake.
    pub wake_placement: WakePlacement,
    /// Maximum closures per pass (`H`, at least 1).
    pub pass_limit: usize,
}

impl ActorOptions {
    /// `actor`: the plain idiom, every wake and yield with default placement.
    pub const fn plain() -> Self {
        Self {
            server_wake: Placement::Default,
            yield_placement: Placement::Default,
            wake_placement: WakePlacement::Default,
            pass_limit: DEFAULT_PASS_LIMIT,
        }
    }

    /// `actor-inline`: server woken into the publisher's run-next slot,
    /// yields home, clients woken remote.
    pub const fn inline() -> Self {
        Self {
            server_wake: Placement::Inline,
            yield_placement: Placement::Home,
            wake_placement: WakePlacement::Remote,
            pass_limit: DEFAULT_PASS_LIMIT,
        }
    }
}

impl Default for ActorOptions {
    fn default() -> Self {
        Self::plain()
    }
}

// ---------------------------------------------------------------------------
// Record
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
    unreachable!("server ran a record without a published request")
}

/// Per-client request record. Allocated once per client, owned by the lock
/// (`Actor::all`), cache-line aligned so the server's completion store does
/// not false-share with another client's record.
#[repr(align(128))]
struct Record<T> {
    /// `PENDING` from publication until the result is written; then
    /// `COMPLETE` (Release by the server, Acquire by the owner).
    state: AtomicU8,
    /// Stack link, written by the owner before the publishing CAS; after the
    /// server's draining swap, the link of the server's FIFO.
    next: AtomicPtr<Record<T>>,
    /// Written by the owner before the publishing CAS, read by the server
    /// after the SeqCst swap that took the record.
    req: UnsafeCell<Request<T>>,
    waker: AtomicWaker,
    /// Cumulative critical-section cycles. Written by the server before
    /// `COMPLETE`, read by the owner (`usage()`).
    usage: AtomicU64,
    /// Link in `Actor::all`; written once in `client()` before the record is
    /// reachable, read only by `Actor::drop`.
    all_next: AtomicPtr<Record<T>>,
}

// SAFETY: the raw pointers inside are dereferenced only under the protocol
// above (owner before publication / after completion, server in between).
unsafe impl<T> Send for Record<T> {}
unsafe impl<T> Sync for Record<T> {}

impl<T> Record<T> {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(COMPLETE),
            next: AtomicPtr::new(ptr::null_mut()),
            req: UnsafeCell::new(Request {
                slot: ptr::null_mut(),
                run: no_request::<T>,
            }),
            waker: AtomicWaker::new(),
            usage: AtomicU64::new(0),
            all_next: AtomicPtr::new(ptr::null_mut()),
        }
    }
}

// ---------------------------------------------------------------------------
// Lock
// ---------------------------------------------------------------------------

/// Server-private state: only the task holding `RUNNING` (or `IDLE`, which
/// hands the lock to nobody but that same task's next poll) touches it.
struct ServerState<T> {
    data: T,
    /// Intrusive FIFO over `Record::next`: drained, not yet served.
    first: *mut Record<T>,
    last: *mut Record<T>,
    passes: u64,
    served: u64,
    parks: u64,
}

/// Server counters over the lock's lifetime ([`Actor::server_stats`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServerStats {
    /// Server tasks spawned (one per span of client lifetime).
    pub spawns: u64,
    /// Non-empty passes (= yields).
    pub passes: u64,
    /// Requests served.
    pub served: u64,
    /// Times the server parked idle.
    pub parks: u64,
}

pub struct Actor<T> {
    /// Request stack, newest first.
    head: CachePadded<AtomicPtr<Record<T>>>,
    /// `ABSENT` / `RUNNING` / `IDLE`.
    server: CachePadded<AtomicU8>,
    srv: CachePadded<UnsafeCell<ServerState<T>>>,
    /// The idle server's waker; see the module docs for the ownership rule.
    server_waker: UnsafeCell<Option<Waker>>,
    /// Clients alive (`client()` minus drops).
    live: AtomicUsize,
    /// Server tasks spawned so far.
    spawns: AtomicU64,
    /// Every record ever handed out (via `Record::all_next`); freed in `Drop`.
    /// The server holds an `Arc` of the lock, so a post-completion wake never
    /// touches a freed record.
    all: AtomicPtr<Record<T>>,
    opts: ActorOptions,
}

// SAFETY: `srv` is touched only by the server task holding `RUNNING` and
// `server_waker` only by the owner defined in the module docs; `T: Send`
// suffices because `T` is only ever touched by that one task.
unsafe impl<T: Send> Send for Actor<T> {}
unsafe impl<T: Send> Sync for Actor<T> {}

impl<T> Actor<T> {
    pub fn options(&self) -> ActorOptions {
        self.opts
    }

    /// Server counters. `&mut self`: only readable once no client and no
    /// server holds the lock (e.g. `Arc::get_mut` after the run).
    pub fn server_stats(&mut self) -> ServerStats {
        let srv = self.srv.get_mut();
        ServerStats {
            spawns: *self.spawns.get_mut(),
            passes: srv.passes,
            served: srv.served,
            parks: srv.parks,
        }
    }

    /// Push onto the request stack. Success is SeqCst: it releases the
    /// record's `req`/`state`/`next` writes to the draining server and takes
    /// part in the Dekker order with `server`.
    fn publish(&self, rec: &Record<T>) {
        let p = rec as *const Record<T> as *mut Record<T>;
        let mut head = self.head.load(Relaxed);
        loop {
            rec.next.store(head, Relaxed);
            match self.head.compare_exchange_weak(head, p, SeqCst, Relaxed) {
                Ok(_) => return,
                Err(h) => head = h,
            }
        }
    }

    /// Move `IDLE` -> `RUNNING` and wake the server. Returns false if the
    /// server was not idle or someone else won the transition.
    fn wake_idle_server(&self) -> bool {
        if self.server.load(SeqCst) != IDLE
            || self
                .server
                .compare_exchange(IDLE, RUNNING, SeqCst, SeqCst)
                .is_err()
        {
            return false;
        }
        // SAFETY: winning IDLE -> RUNNING makes us the slot's owner; the
        // server filled it before storing IDLE (the CAS acquires that) and
        // is not polled again before our wake below.
        let waker = unsafe { (*self.server_waker.get()).take() }
            .expect("idle actor server without a registered waker");
        executor::wake_with(self.opts.server_wake, &waker);
        true
    }

    /// Server only. Take the whole stack and append it to the FIFO in
    /// publication order.
    fn drain(&self, srv: &mut ServerState<T>) {
        let newest = self.head.swap(ptr::null_mut(), SeqCst);
        if newest.is_null() {
            return;
        }
        // Reverse the LIFO chain in place; the swap made every link ours.
        let mut prev: *mut Record<T> = ptr::null_mut();
        let mut p = newest;
        while !p.is_null() {
            // SAFETY: records live as long as the lock; the pusher's `next`
            // store happens-before our SeqCst swap of `head`.
            let r = unsafe { &*p };
            let nx = r.next.load(Relaxed);
            r.next.store(prev, Relaxed);
            prev = p;
            p = nx;
        }
        // `prev` is the oldest request, `newest` the youngest (next = null).
        if srv.last.is_null() {
            srv.first = prev;
        } else {
            // SAFETY: `last` is admitted and unserved, so still ours.
            unsafe { (*srv.last).next.store(prev, Relaxed) };
        }
        srv.last = newest;
    }

    /// Server only. Serve up to `pass_limit` requests in FIFO order.
    fn pass(&self, srv: &mut ServerState<T>) {
        srv.passes += 1;
        for _ in 0..self.opts.pass_limit {
            let p = srv.first;
            if p.is_null() {
                break;
            }
            // SAFETY: the record is admitted and PENDING and lives as long as
            // the lock; its owner's future is pinned and undropped while
            // PENDING (Drop aborts otherwise), so `req.slot` is valid, and
            // only we run it: it left the stack exactly once.
            let rec = unsafe { &*p };
            // The link is the owner's again once COMPLETE is visible.
            srv.first = rec.next.load(Relaxed);
            if srv.first.is_null() {
                srv.last = ptr::null_mut();
            }
            srv.served += 1;
            let req = unsafe { *rec.req.get() };
            let begin = cycles();
            unsafe { (req.run)(req.slot, &mut srv.data) };
            let cs = cycles().wrapping_sub(begin);
            rec.usage.store(rec.usage.load(Relaxed) + cs, Relaxed);
            // Release: publishes the result and the usage store to the
            // owner's Acquire load.
            rec.state.store(COMPLETE, Release);
            rec.waker.wake(self.opts.wake_placement);
        }
    }

    /// Server only, FIFO and stack found empty: park, or carry on if the
    /// Dekker re-check says so, or exit if every client is gone.
    fn idle(&self, cx: &Context<'_>) -> Idle {
        // SAFETY: we hold RUNNING, so the slot is ours until the IDLE store.
        let slot = unsafe { &mut *self.server_waker.get() };
        match slot {
            Some(w) if w.will_wake(cx.waker()) => {}
            _ => *slot = Some(cx.waker().clone()),
        }
        self.server.store(IDLE, SeqCst);
        let work = !self.head.load(SeqCst).is_null();
        let orphaned = self.live.load(SeqCst) == 0;
        if !work && !orphaned {
            return Idle::Parked;
        }
        if self
            .server
            .compare_exchange(IDLE, RUNNING, SeqCst, SeqCst)
            .is_err()
        {
            // A publisher or the last client's drop took the slot and is
            // waking us.
            return Idle::Parked;
        }
        if work {
            return Idle::Work;
        }
        // Orphaned. Drop our waker so the lock does not pin the finished
        // task's allocation.
        // SAFETY: RUNNING again: the slot is ours.
        unsafe { *self.server_waker.get() = None };
        self.server.store(ABSENT, SeqCst);
        // A request that raced the exit either is visible here or its
        // publisher sees ABSENT and spawns a successor.
        if !self.head.load(SeqCst).is_null()
            && self
                .server
                .compare_exchange(ABSENT, RUNNING, SeqCst, SeqCst)
                .is_ok()
        {
            return Idle::Work;
        }
        Idle::Exit
    }
}

impl<T: Send + 'static> Actor<T> {
    pub fn with_options(data: T, opts: ActorOptions) -> Self {
        assert!(opts.pass_limit >= 1, "actor pass_limit must be >= 1");
        let yield_placement = match opts.yield_placement {
            Placement::Inline => Placement::Default,
            p => p,
        };
        Self {
            head: CachePadded::new(AtomicPtr::new(ptr::null_mut())),
            server: CachePadded::new(AtomicU8::new(ABSENT)),
            srv: CachePadded::new(UnsafeCell::new(ServerState {
                data,
                first: ptr::null_mut(),
                last: ptr::null_mut(),
                passes: 0,
                served: 0,
                parks: 0,
            })),
            server_waker: UnsafeCell::new(None),
            live: AtomicUsize::new(0),
            spawns: AtomicU64::new(0),
            all: AtomicPtr::new(ptr::null_mut()),
            opts: ActorOptions {
                yield_placement,
                ..opts
            },
        }
    }

    /// Publisher side, after `publish`: make sure a server will drain.
    fn notify(self: &Arc<Self>) {
        match self.server.load(SeqCst) {
            RUNNING => {}
            IDLE => {
                self.wake_idle_server();
            }
            _ => {
                if self
                    .server
                    .compare_exchange(ABSENT, RUNNING, SeqCst, SeqCst)
                    .is_ok()
                {
                    self.spawn_server();
                }
            }
        }
    }

    /// Called with `RUNNING` claimed from `ABSENT`.
    fn spawn_server(self: &Arc<Self>) {
        self.spawns.fetch_add(1, Relaxed);
        let server = Server {
            lock: Arc::clone(self),
        };
        executor::set_placement_hint(self.opts.server_wake);
        let task = executor::spawn_here(TaskKind::Client, server);
        executor::set_placement_hint(Placement::Default);
        match task {
            Some(t) => t.detach(),
            None => panic!("actor: requests must be issued from an executor worker"),
        }
    }
}

impl<T> Drop for Actor<T> {
    fn drop(&mut self) {
        let mut p = std::mem::replace(self.all.get_mut(), ptr::null_mut());
        while !p.is_null() {
            // SAFETY: every pointer in `all` came from `Box::into_raw` in
            // `client()` and is freed exactly once here; no client and no
            // server is alive (both hold an `Arc`).
            let rec = unsafe { Box::from_raw(p) };
            p = rec.all_next.load(Relaxed);
        }
    }
}

impl<T: Send + 'static> DelegationLock<T> for Actor<T> {
    type Client = ActorClient<T>;

    fn new(data: T) -> Self {
        Self::with_options(data, ActorOptions::plain())
    }

    fn client(self: &Arc<Self>) -> ActorClient<T> {
        let rec = Box::into_raw(Box::new(Record::new()));
        let mut head = self.all.load(Relaxed);
        loop {
            // SAFETY: `rec` is ours until the CAS publishes it to `all`,
            // whose only reader is `Drop`.
            unsafe { (*rec).all_next.store(head, Relaxed) };
            match self.all.compare_exchange_weak(head, rec, Release, Relaxed) {
                Ok(_) => break,
                Err(h) => head = h,
            }
        }
        self.live.fetch_add(1, SeqCst);
        ActorClient {
            lock: Arc::clone(self),
            // SAFETY: `Box::into_raw` never returns null.
            rec: unsafe { NonNull::new_unchecked(rec) },
        }
    }

    fn name() -> &'static str {
        "actor"
    }
}

// ---------------------------------------------------------------------------
// Server task
// ---------------------------------------------------------------------------

enum Idle {
    /// Parked; a publisher or the last client's drop will wake us.
    Parked,
    /// Back to `RUNNING` with work to drain.
    Work,
    /// Back to `ABSENT`: finish the task.
    Exit,
}

struct Server<T> {
    lock: Arc<Actor<T>>,
}

impl<T: Send + 'static> Future for Server<T> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let lock = &*self.lock;
        loop {
            let begin = cycles();
            {
                // SAFETY: we hold RUNNING (see `Actor::srv`); a task is polled
                // by one thread at a time and async-task orders its polls;
                // successive server tasks are ordered by the SeqCst
                // ABSENT/RUNNING transitions. The borrow ends before `idle`
                // can store ABSENT, after which a successor may start.
                let srv = unsafe { &mut *lock.srv.get() };
                lock.drain(srv);
                if !srv.first.is_null() {
                    lock.pass(srv);
                    if let Some(w) = executor::worker_id() {
                        record_combining(w, cycles().wrapping_sub(begin));
                    }
                    // Yield: the deferred schedule after this `Pending`
                    // consumes the hint (nothing else is scheduled from
                    // this thread before we return).
                    executor::set_placement_hint(lock.opts.yield_placement);
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
            }
            match lock.idle(cx) {
                Idle::Parked => {
                    // SAFETY: IDLE (unlike ABSENT) lets no other task in; a
                    // wake of this task is deferred until this poll returns.
                    unsafe { (*lock.srv.get()).parks += 1 };
                    return Poll::Pending;
                }
                Idle::Work => continue,
                Idle::Exit => return Poll::Ready(()),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Client and the `run` future
// ---------------------------------------------------------------------------

pub struct ActorClient<T> {
    lock: Arc<Actor<T>>,
    /// Owned by `lock.all`; valid while `lock` is alive.
    rec: NonNull<Record<T>>,
}

// SAFETY: `rec` is only dereferenced by the owning task (or by the server
// through the publication protocol); `Record` is `Sync`.
unsafe impl<T: Send> Send for ActorClient<T> {}

impl<T> ActorClient<T> {
    #[inline]
    fn rec(&self) -> &Record<T> {
        // SAFETY: `lock` (held by `self`) owns the record.
        unsafe { self.rec.as_ref() }
    }
}

impl<T> Drop for ActorClient<T> {
    fn drop(&mut self) {
        // Last client: an idle server must wake up to exit (module docs,
        // "Lifetime"). A running server sees the count when it idles.
        if self.lock.live.fetch_sub(1, SeqCst) == 1 {
            self.lock.wake_idle_server();
        }
    }
}

impl<T: Send + 'static> LockClient<T> for ActorClient<T> {
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
        self.rec().usage.load(Relaxed)
    }
}

enum Slot<R, F> {
    Pending(F),
    Done(R),
    Empty,
}

/// Type-erased entry point the server calls: consumes the closure, stores
/// the result in place.
unsafe fn run_slot<T, R, F: FnOnce(&mut T) -> R>(slot: *mut (), data: &mut T) {
    // SAFETY: `slot` points into a pinned, live `Run` whose record is
    // PENDING; the server has exclusive access until it stores COMPLETE.
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
    Done,
}

/// Future returned by `LockClient::run`; `!Unpin` because the server holds
/// a raw pointer to `slot` while `stage == Published`.
pub struct Run<'a, T, R, F> {
    client: &'a mut ActorClient<T>,
    slot: UnsafeCell<Slot<R, F>>,
    stage: Stage,
    _pin: PhantomPinned,
}

impl<T, R, F> Future for Run<'_, T, R, F>
where
    T: Send + 'static,
    F: FnOnce(&mut T) -> R,
{
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<R> {
        // SAFETY: we never move out of `this`; `slot`'s address is stable for
        // the future's lifetime because it is pinned.
        let this = unsafe { self.get_unchecked_mut() };
        let lock = &this.client.lock;
        let rec = this.client.rec();
        let placement = lock.opts.wake_placement;
        match this.stage {
            Stage::Unpublished => {
                rec.waker.register(cx.waker(), placement);
                // SAFETY: not yet published, so the server does not read it.
                unsafe {
                    *rec.req.get() = Request {
                        slot: this.slot.get().cast::<()>(),
                        run: run_slot::<T, R, F>,
                    };
                }
                rec.state.store(PENDING, Relaxed);
                this.stage = Stage::Published;
                lock.publish(rec);
                lock.notify();
                Poll::Pending
            }
            Stage::Published => {
                if rec.state.load(Acquire) == COMPLETE {
                    return Poll::Ready(this.finish());
                }
                rec.waker.register(cx.waker(), placement);
                // Re-check after registering: a completion that raced the
                // registration is either seen here or delivered as a wake.
                if rec.state.load(Acquire) == COMPLETE {
                    return Poll::Ready(this.finish());
                }
                Poll::Pending
            }
            Stage::Done => panic!("`run` future polled after completion"),
        }
    }
}

impl<T, R, F> Run<'_, T, R, F> {
    /// Called after observing `COMPLETE` with Acquire.
    fn finish(&mut self) -> R {
        self.stage = Stage::Done;
        // SAFETY: the server released its access with the COMPLETE store.
        match std::mem::replace(unsafe { &mut *self.slot.get() }, Slot::Empty) {
            Slot::Done(r) => r,
            _ => unreachable!("completed request without result"),
        }
    }
}

impl<T, R, F> Drop for Run<'_, T, R, F> {
    fn drop(&mut self) {
        if self.stage == Stage::Published && self.client.rec().state.load(Acquire) != COMPLETE {
            // The server may still dereference our slot. Cancellation after
            // publication is outside the contract (lock.rs).
            eprintln!("coro_delegation: actor `run` future dropped while published; aborting");
            std::process::abort();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::Executor;
    use crate::workload::spin_cycles;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::{Duration, Instant};

    /// Deadline for one executor run; a lost request or server wake-up
    /// hangs the run instead of failing it.
    const DEADLINE: Duration = Duration::from_secs(60);

    /// Protected value that detects overlapping critical sections.
    #[derive(Default)]
    struct Guarded {
        count: u64,
        inside: bool,
        overlaps: u64,
    }

    fn variants() -> Vec<(String, ActorOptions)> {
        let mut out = Vec::new();
        for (name, base) in [
            ("actor", ActorOptions::plain()),
            ("actor-inline", ActorOptions::inline()),
        ] {
            for wake in [
                WakePlacement::Default,
                WakePlacement::Remote,
                WakePlacement::Home,
            ] {
                for pass_limit in [1, DEFAULT_PASS_LIMIT] {
                    out.push((
                        format!("{name} wake={wake:?} h={pass_limit}"),
                        ActorOptions {
                            wake_placement: wake,
                            pass_limit,
                            ..base
                        },
                    ));
                }
            }
        }
        out
    }

    fn wait_for(what: &str, cond: impl Fn() -> bool) {
        let t0 = Instant::now();
        while !cond() {
            assert!(t0.elapsed() < DEADLINE, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    struct Outcome {
        count: u64,
        overlaps: u64,
        usage: Vec<u64>,
        stats: ServerStats,
    }

    /// `clients` tasks x `ops` requests on a real executor. Each request is
    /// a non-atomic read-modify-write that counts overlaps and returns the
    /// new count; between requests a client spins up to `max_gap` cycles
    /// (0 = sustained; large = the server idles and is re-woken often).
    /// Then: wait for the server to exit with the last client, read the
    /// final value through a fresh client (respawn), wait for exit again.
    fn stress(
        opts: ActorOptions,
        workers: usize,
        clients: usize,
        ops: u64,
        cs: u64,
        max_gap: u64,
    ) -> Outcome {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut lock = Arc::new(Actor::with_options(Guarded::default(), opts));
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
            wait_for("server exit after the last client", || {
                lock.server.load(SeqCst) == ABSENT
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
            wait_for("server exit after the final read", || {
                lock.server.load(SeqCst) == ABSENT
            });
            // The exited server's task has dropped its `Arc`.
            wait_for("server Arc release", || Arc::strong_count(&lock) == 1);
            exec.shutdown();
            let stats = Arc::get_mut(&mut lock)
                .expect("exited server kept the lock")
                .server_stats();
            let _ = tx.send(Outcome {
                count,
                overlaps,
                usage,
                stats,
            });
        });
        match rx.recv_timeout(DEADLINE) {
            Ok(o) => o,
            Err(RecvTimeoutError::Timeout) => {
                panic!("actor run did not finish within {DEADLINE:?}: lost request or wake-up")
            }
            Err(RecvTimeoutError::Disconnected) => panic!("actor run panicked (see above)"),
        }
    }

    fn check(name: &str, o: &Outcome, clients: usize, ops: u64, cs: u64) {
        assert_eq!(o.overlaps, 0, "{name}: overlapping critical sections");
        assert_eq!(
            o.count,
            clients as u64 * ops,
            "{name}: lost or duplicated request"
        );
        for u in &o.usage {
            assert!(*u >= ops * cs, "{name}: usage {u} below the spin floor");
        }
        let s = &o.stats;
        assert_eq!(s.spawns, 2, "{name}: one server per client lifetime");
        assert_eq!(s.served, o.count + 1, "{name}: served count");
        assert!(
            s.passes >= s.served.div_ceil(64) && s.passes <= s.served,
            "{name}: {s:?}"
        );
    }

    #[test]
    fn mutual_exclusion_and_completion_sustained() {
        let (clients, ops, cs) = (16, 2000, 200);
        for (name, opts) in variants() {
            let o = stress(opts, 4, clients, ops, cs, 0);
            check(&name, &o, clients, ops, cs);
        }
    }

    /// Random gaps between requests, so the server drains, idles and is
    /// re-woken (with `server_wake = Inline`, hopping to the publisher's
    /// worker) over and over; any lost wake-up hangs the run. Dense: many
    /// clients, short gaps (publishers race a draining server). Sparse: few
    /// clients, long gaps (the server idles between most requests, even
    /// when slowed down by instrumentation).
    #[test]
    fn no_lost_request_across_idle_transitions() {
        let cs = 100;
        for (name, opts) in variants() {
            let (clients, ops) = (8, 1500);
            let o = stress(opts, 4, clients, ops, cs, 40_000);
            check(&name, &o, clients, ops, cs);
            let (clients, ops) = (3, 400);
            let o = stress(opts, 4, clients, ops, cs, 400_000);
            check(&name, &o, clients, ops, cs);
            // Default and home client wakes can leave a served client on
            // the server's worker, ahead of the server in its queue, whose
            // gap then delays the server until the next request is already
            // there: few parks. Remote wakes spread the clients, so there
            // the server must have parked often.
            if opts.wake_placement == WakePlacement::Remote {
                assert!(o.stats.parks >= 100, "{name}: {:?}", o.stats);
            }
        }
    }

    /// One worker: clients are polled in spawn order and publish on their
    /// first poll; the server (spawned behind them) serves in publication
    /// order.
    #[test]
    fn serves_in_publication_order() {
        for (name, base) in [
            ("actor", ActorOptions::plain()),
            ("actor-inline", ActorOptions::inline()),
        ] {
            let opts = ActorOptions {
                pass_limit: 3,
                ..base
            };
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let n = 12;
                let lock = Arc::new(Actor::with_options(Vec::<usize>::new(), opts));
                let exec = Executor::with_balance_interval(1, 0);
                let tasks: Vec<_> = (0..n)
                    .map(|i| {
                        let lock = Arc::clone(&lock);
                        exec.spawn_on(0, TaskKind::Client, async move {
                            lock.client()
                                .run(move |log: &mut Vec<usize>| log.push(i))
                                .await;
                        })
                    })
                    .collect();
                exec.block_on(async {
                    for t in tasks {
                        t.await;
                    }
                });
                let read = exec.spawn(TaskKind::Client, {
                    let lock = Arc::clone(&lock);
                    async move { lock.client().run(|log: &mut Vec<usize>| log.clone()).await }
                });
                let log = exec.block_on(read);
                exec.shutdown();
                let _ = tx.send(log);
            });
            let log = rx
                .recv_timeout(DEADLINE)
                .expect("ordered run did not finish");
            assert_eq!(log, (0..12).collect::<Vec<_>>(), "{name}");
        }
    }
}
