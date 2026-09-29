//! `cfl`: a usage-fair queue lock *without* delegation, an async adaptation
//! of CFL (Park & Eom, PPoPP'24; ShflLock family) for the M:N executor. The
//! thread-based reference is the port in
//! `crates/libdlock/src/dlock2/cfl.rs` (lock word with `locked` /
//! `no_stealing`, MCS queue, the waiter that will get the lock next spins on
//! the lock word and shuffles the queue behind itself meanwhile).
//!
//! Every critical section runs on its client's own task and worker, as in
//! [`super::dispatch`] and [`super::dispatch_pq`]. What differs from
//! `dispatch-pq` is where the policy work and the wake happen:
//!
//! - **Queue.** An MCS-style intrusive list of per-client [`Node`]s: enqueue
//!   swaps `tail` and links `prev.next`. The node at the front is the
//!   *head*, the next owner. The head acquires the lock word by CAS and then
//!   passes headship to its `next` (the MCS handoff moves the queue front,
//!   not the lock). An uncontended acquire is one CAS on the word; it
//!   succeeds only while `NO_STEAL` is clear, and `NO_STEAL` is set while a
//!   head exists. One race weakens that last clause, and it affects fairness
//!   only (mutual exclusion is the `LOCKED` bit under a full-word CAS). A
//!   head that leaves an empty queue clears `NO_STEAL` after its tail CAS.
//!   That clear can land after a new first-enqueuer set `NO_STEAL`, and the
//!   queue is then non-empty with `NO_STEAL` clear. A spinning head
//!   re-asserts it the first time it sees `LOCKED` without it. A parked
//!   head only re-asserts once it is polled again, and a head's own CAS
//!   (`0 -> LOCKED`) keeps it clear. So until some head polls and sees the
//!   word held, uncontended acquisitions can keep overtaking the queue. The
//!   number of such steals is not bounded by the protocol; it depends on
//!   when the executor polls the head. `CflStats::fast` counts them.
//! - **Shuffling off the critical path** ([`Shuffle::Usage`]). While the
//!   owner runs, the head examines the waiters behind it and picks its
//!   successor: the smallest scheduling key, FIFO among equals. The key is
//!   the client's cumulative charged critical-section cycles at enqueue
//!   ([`UsageNode`] accounting as in FC-PQ / dispatch-pq; a never-served
//!   client enters at the lock's running mean cost per request). At its
//!   acquisition the head splices that node right behind itself and passes
//!   headship to it. Only the head writes interior links, and a node whose
//!   `next` is still null (the tail, or a node an enqueuer is linking) is
//!   never moved, because an enqueuer may be writing its `next`. Each splice
//!   moves one node to the front, and that node becomes head at once, so the
//!   waiters strictly behind the head always stay in arrival order.
//!   [`Shuffle::Off`] is MCS FIFO.
//! - **Starvation bound.** A waiter's skip count is the number of handoffs
//!   (acquisitions by a queue head) since it enqueued: the global handoff
//!   tick minus its tick at enqueue, so no per-handoff write to every
//!   waiter is needed. Waiters behind the head are in arrival order, so the
//!   oldest one is the head's current `next`. Once its skip count exceeds
//!   `starvation_clamp` it receives headship ahead of cheaper waiters
//!   (default [`DEFAULT_STARVATION_CLAMP`]; 0 = off). It is the only waiter
//!   checked, and it is served at the next handoff, so a waiter waits at
//!   most `clamp + 2` handoffs plus one per older starving waiter.
//! - **Pre-wake** (`prewake`). A node is woken when it *becomes head*, i.e.
//!   when its predecessor acquires the lock, not at release. Its scheduling
//!   latency therefore overlaps the owner's critical section. The head then
//!   waits for the lock word by spinning inside its poll for at most
//!   `head_spin_cycles`. If the word is still held after that, it parks:
//!   registers its waker, publishes itself in `parked_head`, and re-checks
//!   the word. The releaser clears the word and wakes a parked head. With
//!   `prewake` off, the owner wakes its successor only after its release,
//!   as `dispatch` does.
//! - **Scan policy** ([`Scan`]). `Full`: before each acquisition attempt the
//!   head finishes examining every visible, movable waiter, so every
//!   handoff is usage-ordered. When the head arrives after the release,
//!   that scan is on the critical path. `Overlap` is the CFL rule: examine
//!   waiters only while the word is held, in chunks between lock checks,
//!   and take the lock as soon as it is free with whatever has been
//!   examined.
//!
//! Accounting: the owner charges `cycles()` around its own closure as
//! `key + cs`, where `key` is the enqueue-time key (or its own usage on the
//! uncontended path, as in dispatch-pq), and updates the running mean
//! totals. With `record_handoffs`, [`CflStats`] records where each handoff's
//! cycles go. [`take_handoff_stats`] publishes them.
//!
//! ## Memory model and no lost wakeup
//!
//! - Word: `LOCKED | NO_STEAL`. Uncontended acquire is CAS `0 -> LOCKED`
//!   (Acquire). The head acquires by CAS `w -> w | LOCKED` from a `w`
//!   without `LOCKED` (Acquire). Release is `fetch_and(!LOCKED)` (SeqCst,
//!   which includes Release). Each acquisition thereby synchronises with the
//!   previous release, which orders the critical sections.
//! - Headship: the granter writes the successor's `grant_tsc`, then
//!   `status = HEAD` (Release). The waiter reads `status` with Acquire. It
//!   therefore sees the granter's splice (Relaxed link stores before the
//!   Release) and, transitively, every earlier head's link writes. Keys and
//!   ticks are written by their owner before its `tail` swap (AcqRel). A
//!   head reaches a node through an Acquire load of a `next` written with
//!   Release by the node's enqueuer, or through a splice it has
//!   synchronised with.
//! - Headship wake vs. registration: [`AtomicWaker`] (register, then
//!   re-check `status`; the granter stores `status` before `wake`). A wake
//!   that races a registration is delivered by the registrant.
//! - Head park vs. release, a Dekker pair on two SeqCst locations. The head
//!   does `register(waker)`, then `parked_head.store(me)`, then
//!   `word.load()`. It returns `Pending` only if that load sees `LOCKED`.
//!   The releaser does `word.fetch_and(!LOCKED)`, then `parked_head.load()`,
//!   and if the load is non-null it swaps the slot to null and wakes that
//!   node. In the SeqCst total order, if the head's load saw the word
//!   before the release, then the head's store precedes the releaser's
//!   load, so the releaser finds the head (or a later store). Other swaps
//!   only take the entry to wake it, so every wake of the head follows its
//!   registration. Such a wake either finds the head's waker or has been
//!   preceded by another delivered wake, and the head's next poll re-parks
//!   with a fresh registration or finds the word free. Test:
//!   `no_lost_wakeup_across_head_park_and_release`.
//! - A late or duplicate wake (a stale pre-wake, a releaser that took a
//!   `parked_head` entry the head has already withdrawn) is only a spurious
//!   poll; every poll re-checks its condition.
//!
//! ## Node lifetime
//!
//! Nodes are allocated once per client in [`DelegationLock::client`] and
//! are owned by the lock (freed when the lock is dropped). A granter or
//! releaser may touch a node after its owner has moved on (a late `wake`,
//! the `parked_head` slot), so client-owned memory would be a
//! use-after-free. No per-request allocation. Dropping a `run` future after
//! it has enqueued is not supported (cancellation after publication is out
//! of scope, `lock.rs`); it aborts the process unless the thread is already
//! panicking. `Executor::shutdown` drops queued runnables, so shutting the
//! executor down while a client is still queued would hit that abort. The
//! harness and the tests await every client task before shutdown. A
//! closure that panics leaves the lock held.

use std::cell::UnsafeCell;
use std::future::Future;
use std::pin::Pin;
use std::ptr::{self, NonNull};
use std::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release, SeqCst};
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, AtomicU8};
use std::sync::Arc;
use std::task::{Context, Poll};

use crossbeam_utils::CachePadded;
use parking_lot::Mutex;

use super::fc::{AtomicWaker, WakePlacement};
use super::fc_pq::{UsageNode, WAIT_BUCKETS};
use crate::lock::{cycles, DelegationLock, LockClient};
use crate::stats;

/// Default starvation clamp, in handoffs (the dispatch-pq finding: clamps
/// below the ~36-handoff light wait promote everything and make the queue
/// FIFO; 256 bounds the heavy wait without binding).
pub const DEFAULT_STARVATION_CLAMP: u64 = 256;

/// Default head spin budget before parking, TSC cycles: about one heavy
/// critical section of the benchmark (8 x 1 000 cycles of spin plus one
/// insert), so that a head that arrives during a heavy critical section
/// does not park. In an exploratory sweep (W = 8, b31, sustained, n = 1),
/// 2 000 (2x light) parked 13 % of heads and ran 0.288 Mops/s; 4 000 /
/// 8 000 / 16 000 ran 0.301 / 0.308 / 0.308.
pub const DEFAULT_HEAD_SPIN_CYCLES: u64 = 8_000;

/// Waiters examined per spin-loop iteration between lock-word checks.
const SCAN_CHUNK: usize = 4;

const LOCKED: u32 = 1;
const NO_STEAL: u32 = 2;

const WAIT: u8 = 0;
const HEAD: u8 = 1;

/// Successor selection by the queue head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shuffle {
    /// No reordering: MCS FIFO.
    Off,
    /// Minimum enqueue-time charged usage among the waiters behind the head.
    Usage,
}

/// When the head examines waiters ([`Shuffle::Usage`] only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scan {
    /// Finish examining every visible, movable waiter before each
    /// acquisition attempt.
    Full,
    /// Examine waiters only while the lock word is held (CFL rule).
    Overlap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CflOptions {
    pub shuffle: Shuffle,
    pub scan: Scan,
    /// Wake a node when it becomes head (`true`) or after the release that
    /// makes it the owner's successor free to take the lock (`false`).
    pub prewake: bool,
    /// Handoffs a waiter may watch go to others before it receives headship
    /// regardless of usage; 0 = off.
    pub starvation_clamp: u64,
    /// Head spin budget per poll before parking, TSC cycles.
    pub head_spin_cycles: u64,
    /// Placement of every wake the lock issues (pre-wake, release wake).
    pub wake_placement: WakePlacement,
    /// Record [`CflStats`] while `stats::recording()` is on.
    pub record_handoffs: bool,
}

impl Default for CflOptions {
    fn default() -> Self {
        Self {
            shuffle: Shuffle::Usage,
            scan: Scan::Full,
            prewake: true,
            starvation_clamp: DEFAULT_STARVATION_CLAMP,
            head_spin_cycles: DEFAULT_HEAD_SPIN_CYCLES,
            wake_placement: WakePlacement::Home,
            record_handoffs: false,
        }
    }
}

/// Where acquisitions came from and where their cycles go, summed over
/// clients (TSC cycles). A handoff is an acquisition by a queue head, timed
/// from the previous owner's critical-section end to this owner's
/// critical-section start: `release + react + pass`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CflStats {
    /// Uncontended acquisitions (CAS on a free word, no queue).
    pub fast: u64,
    /// Timed acquisitions by a queue head.
    pub handoffs: u64,
    /// Previous owner: critical-section end to lock word cleared
    /// (accounting, release store).
    pub release_cycles: u64,
    /// Lock word cleared to the head's successful CAS.
    pub react_cycles: u64,
    /// CAS to critical-section start: handoff tick, successor choice and
    /// splice, headship pass, pre-wake call.
    pub pass_cycles: u64,
    /// Handoffs by how the head got the lock, and their `react` cycles:
    /// `spin`, it saw the word held in the acquiring poll and spun until it
    /// was cleared; `late`, the word was already free at the start of the
    /// acquiring poll and the head had never parked in this headship (the
    /// pre-wake came too late, or pre-wake is off); `woken`, the head had
    /// parked and a releaser woke it.
    pub acq_spin: u64,
    pub acq_spin_react: u64,
    pub acq_late: u64,
    pub acq_late_react: u64,
    pub acq_woken: u64,
    pub acq_woken_react: u64,
    /// Granted heads, the grant-to-first-poll-as-head cycles (the new
    /// head's scheduling latency), and how many of them found the word
    /// still held at that first poll (arrived during the critical section).
    pub grants: u64,
    pub grant_to_poll_cycles: u64,
    pub early_heads: u64,
    /// Cycles spent examining waiters (only calls that examined ≥ 1), the
    /// part of them after the lock word was cleared (on the critical path),
    /// and waiters examined.
    pub scan_cycles: u64,
    pub scan_on_path_cycles: u64,
    pub scan_visited: u64,
    /// Handoffs whose successor was spliced forward (not the oldest waiter).
    pub moves: u64,
    /// Handoffs that passed headship to the oldest waiter because its skip
    /// count exceeded the clamp.
    pub promoted: u64,
    /// Head spin-loop cycles, head parks, park attempts withdrawn because
    /// the re-check found the word free, and wakes of a parked head issued
    /// by releasers.
    pub spin_cycles: u64,
    pub parks: u64,
    pub park_rechecks: u64,
    pub head_wakes: u64,
    /// Waits in handoffs (acquisition tick minus enqueue tick) of queued
    /// acquisitions; the last bucket is open-ended.
    pub max_wait: u64,
    pub wait_hist: [u64; WAIT_BUCKETS],
}

impl Default for CflStats {
    fn default() -> Self {
        Self {
            fast: 0,
            handoffs: 0,
            release_cycles: 0,
            react_cycles: 0,
            pass_cycles: 0,
            acq_spin: 0,
            acq_spin_react: 0,
            acq_late: 0,
            acq_late_react: 0,
            acq_woken: 0,
            acq_woken_react: 0,
            grants: 0,
            grant_to_poll_cycles: 0,
            early_heads: 0,
            scan_cycles: 0,
            scan_on_path_cycles: 0,
            scan_visited: 0,
            moves: 0,
            promoted: 0,
            spin_cycles: 0,
            parks: 0,
            park_rechecks: 0,
            head_wakes: 0,
            max_wait: 0,
            wait_hist: [0; WAIT_BUCKETS],
        }
    }
}

impl CflStats {
    fn record_wait(&mut self, wait: u64) {
        self.max_wait = self.max_wait.max(wait);
        self.wait_hist[(wait as usize).min(WAIT_BUCKETS - 1)] += 1;
    }

    fn add(&mut self, o: &CflStats) {
        self.fast += o.fast;
        self.handoffs += o.handoffs;
        self.release_cycles += o.release_cycles;
        self.react_cycles += o.react_cycles;
        self.pass_cycles += o.pass_cycles;
        self.acq_spin += o.acq_spin;
        self.acq_spin_react += o.acq_spin_react;
        self.acq_late += o.acq_late;
        self.acq_late_react += o.acq_late_react;
        self.acq_woken += o.acq_woken;
        self.acq_woken_react += o.acq_woken_react;
        self.grants += o.grants;
        self.grant_to_poll_cycles += o.grant_to_poll_cycles;
        self.early_heads += o.early_heads;
        self.scan_cycles += o.scan_cycles;
        self.scan_on_path_cycles += o.scan_on_path_cycles;
        self.scan_visited += o.scan_visited;
        self.moves += o.moves;
        self.promoted += o.promoted;
        self.spin_cycles += o.spin_cycles;
        self.parks += o.parks;
        self.park_rechecks += o.park_rechecks;
        self.head_wakes += o.head_wakes;
        self.max_wait = self.max_wait.max(o.max_wait);
        for (a, b) in self.wait_hist.iter_mut().zip(o.wait_hist.iter()) {
            *a += b;
        }
    }

    /// Queued acquisitions counted in the wait histogram.
    pub fn queued(&self) -> u64 {
        self.wait_hist.iter().sum()
    }
}

/// Stats of the last dropped lock that recorded anything.
static LAST_STATS: Mutex<Option<CflStats>> = Mutex::new(None);

/// Take the [`CflStats`] of the most recently dropped [`Cfl`] that recorded
/// any (the harness drops the lock when `run_raw` returns).
pub fn take_handoff_stats() -> Option<CflStats> {
    LAST_STATS.lock().take()
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

/// What a scanning head reads: link, key, skip-count tick. Written by the
/// owner before its tail swap, `next` also by its successor's enqueuer
/// (once) and by the head (splices).
struct Link {
    next: AtomicPtr<Node>,
    /// Scheduling key and accounting base of the queued request.
    key: AtomicU64,
    /// Handoff tick at enqueue.
    enq_tick: AtomicU64,
}

/// What the granter and releasers write.
struct Signal {
    status: AtomicU8,
    /// `record_handoffs`: grant timestamp (0 = not granted by a head).
    grant_tsc: AtomicU64,
    waker: AtomicWaker,
}

/// Owner-only state (its own lines: scanners never read it).
struct Own {
    usage: AtomicU64,
    served: AtomicU64,
    head: UnsafeCell<HeadState>,
    stats: UnsafeCell<CflStats>,
}

/// The head's scan progress and per-headship flags, reset at enqueue.
struct HeadState {
    /// Last waiter examined (the node itself before the first).
    resume: *const Node,
    /// Best successor found so far, its predecessor, and its key.
    best: *const Node,
    best_prev: *const Node,
    best_key: u64,
    /// No poll as head yet in this headship.
    first_poll: bool,
    /// Parked at least once in this headship.
    parked: bool,
    /// Pre-wake off: successor to wake after the release.
    successor: *const Node,
}

impl HeadState {
    fn new(me: *const Node) -> Self {
        Self {
            resume: me,
            best: ptr::null(),
            best_prev: ptr::null(),
            best_key: u64::MAX,
            first_poll: true,
            parked: false,
            successor: ptr::null(),
        }
    }
}

/// Per-client queue node, owned by the lock.
pub struct Node {
    link: CachePadded<Link>,
    signal: CachePadded<Signal>,
    own: CachePadded<Own>,
}

// SAFETY: cross-thread fields are atomics or the `AtomicWaker`; `own.head`
// and `own.stats` are touched only by the owning client's task (one poll at
// a time), and the raw pointers in `HeadState` refer to lock-owned nodes.
unsafe impl Send for Node {}
unsafe impl Sync for Node {}

impl Node {
    fn new() -> Self {
        let n = Node {
            link: CachePadded::new(Link {
                next: AtomicPtr::new(ptr::null_mut()),
                key: AtomicU64::new(0),
                enq_tick: AtomicU64::new(0),
            }),
            signal: CachePadded::new(Signal {
                status: AtomicU8::new(WAIT),
                grant_tsc: AtomicU64::new(0),
                waker: AtomicWaker::new(),
            }),
            own: CachePadded::new(Own {
                usage: AtomicU64::new(0),
                served: AtomicU64::new(0),
                head: UnsafeCell::new(HeadState::new(ptr::null())),
                stats: UnsafeCell::new(CflStats::default()),
            }),
        };
        n
    }

    /// Owner only.
    #[allow(clippy::mut_from_ref)]
    fn head_state(&self) -> &mut HeadState {
        // SAFETY: owner-only (see `Node`'s Send/Sync note); callers never
        // hold two references at once.
        unsafe { &mut *self.own.head.get() }
    }

    /// Owner only.
    #[allow(clippy::mut_from_ref)]
    fn stats(&self) -> &mut CflStats {
        // SAFETY: as `head_state`.
        unsafe { &mut *self.own.stats.get() }
    }
}

impl UsageNode for Node {
    fn usage(&self) -> u64 {
        self.own.usage.load(Relaxed)
    }
    fn served(&self) -> u64 {
        self.own.served.load(Relaxed)
    }
    fn charge(&self, usage: u64) {
        self.own.usage.store(usage, Relaxed);
        self.own
            .served
            .store(self.own.served.load(Relaxed) + 1, Relaxed);
    }
    fn set_usage(&self, usage: u64) {
        self.own.usage.store(usage, Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Lock
// ---------------------------------------------------------------------------

/// The lock word and the parked-head slot.
struct Word {
    word: AtomicU32,
    parked_head: AtomicPtr<Node>,
}

/// Owner-written, read by enqueuers and heads.
struct Acct {
    /// Running totals over served requests (newcomer mean).
    usage_total: AtomicU64,
    served_total: AtomicU64,
    /// Acquisitions by a queue head so far: the skip-count clock.
    handoffs: AtomicU64,
    /// `record_handoffs`: last critical-section end and release timestamps.
    last_cs_end: AtomicU64,
    last_unlock: AtomicU64,
}

pub struct Cfl<T> {
    word: CachePadded<Word>,
    tail: CachePadded<AtomicPtr<Node>>,
    acct: CachePadded<Acct>,
    data: UnsafeCell<T>,
    opts: CflOptions,
    /// Every node ever handed to a client; freed on drop.
    nodes: Mutex<Vec<NonNull<Node>>>,
    /// `record_handoffs`: stats of dropped clients.
    stats: Mutex<CflStats>,
}

// SAFETY: `data` is accessed only by the lock owner; nodes are reached only
// through the queue protocol; everything else is atomic or mutex-guarded.
unsafe impl<T: Send> Send for Cfl<T> {}
unsafe impl<T: Send> Sync for Cfl<T> {}

impl<T> Drop for Cfl<T> {
    fn drop(&mut self) {
        for n in self.nodes.get_mut().drain(..) {
            // SAFETY: every client is gone (they hold an `Arc` to the lock),
            // so no future references a node; each came from `Box::into_raw`.
            drop(unsafe { Box::from_raw(n.as_ptr()) });
        }
        let s = *self.stats.get_mut();
        if s != CflStats::default() {
            *LAST_STATS.lock() = Some(s);
        }
    }
}

/// How a head got the lock (see [`CflStats::acq_spin`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum HeadAcq {
    Spin,
    Late,
    Woken,
}

/// Instrumentation snapshot taken at acquisition.
#[derive(Clone, Copy)]
struct Timing {
    t_acq: u64,
    prev_end: u64,
    prev_unlock: u64,
    /// `None` for the uncontended path.
    head: Option<(HeadAcq, u64)>,
}

impl<T: Send + 'static> Cfl<T> {
    pub fn with_options(data: T, opts: CflOptions) -> Self {
        Cfl {
            word: CachePadded::new(Word {
                word: AtomicU32::new(0),
                parked_head: AtomicPtr::new(ptr::null_mut()),
            }),
            tail: CachePadded::new(AtomicPtr::new(ptr::null_mut())),
            acct: CachePadded::new(Acct {
                usage_total: AtomicU64::new(0),
                served_total: AtomicU64::new(0),
                handoffs: AtomicU64::new(0),
                last_cs_end: AtomicU64::new(0),
                last_unlock: AtomicU64::new(0),
            }),
            data: UnsafeCell::new(data),
            opts,
            nodes: Mutex::new(Vec::new()),
            stats: Mutex::new(CflStats::default()),
        }
    }

    pub fn options(&self) -> CflOptions {
        self.opts
    }
}

impl<T> Cfl<T> {
    #[inline]
    fn recording(&self) -> bool {
        self.opts.record_handoffs && stats::recording()
    }

    #[inline]
    fn try_fast(&self) -> bool {
        self.word
            .word
            .compare_exchange(0, LOCKED, Acquire, Relaxed)
            .is_ok()
    }

    /// Queue `node`. Returns `true` if it became head at once (empty queue).
    fn enqueue(&self, node: &Node) -> bool {
        let base = if node.served() == 0 {
            let n = self.acct.served_total.load(Relaxed);
            if n > 0 {
                self.acct.usage_total.load(Relaxed) / n
            } else {
                node.usage()
            }
        } else {
            node.usage()
        };
        let me = node as *const Node as *mut Node;
        node.link.next.store(ptr::null_mut(), Relaxed);
        node.link.key.store(base, Relaxed);
        node.link
            .enq_tick
            .store(self.acct.handoffs.load(Relaxed), Relaxed);
        node.signal.status.store(WAIT, Relaxed);
        node.signal.grant_tsc.store(0, Relaxed);
        *node.head_state() = HeadState::new(me);
        let prev = self.tail.swap(me, AcqRel);
        if prev.is_null() {
            self.word.word.fetch_or(NO_STEAL, Acquire);
            true
        } else {
            // SAFETY: `prev` is a queued node; nodes live as long as the lock.
            unsafe { &*prev }.link.next.store(me, Release);
            false
        }
    }

    /// Examine up to `max` waiters behind `head` that this headship has not
    /// examined yet; returns how many it examined.
    ///
    /// SAFETY: the caller is the queue head (the only writer of interior
    /// links) and `hs` is its head state.
    unsafe fn scan(head: &Node, hs: &mut HeadState, max: usize) -> u64 {
        let head_ptr: *const Node = head;
        let mut n = 0;
        while (n as usize) < max {
            let resume = &*hs.resume;
            let cur = resume.link.next.load(Acquire);
            if cur.is_null() {
                break;
            }
            let c = &*cur;
            if hs.resume == head_ptr {
                // The oldest waiter: the default successor, never moved.
                hs.best = cur;
                hs.best_prev = head_ptr;
                hs.best_key = c.link.key.load(Relaxed);
            } else {
                // Only a node whose `next` is set may be unlinked later.
                if c.link.next.load(Acquire).is_null() {
                    break;
                }
                let k = c.link.key.load(Relaxed);
                if k < hs.best_key {
                    hs.best = cur;
                    hs.best_prev = hs.resume;
                    hs.best_key = k;
                }
            }
            hs.resume = cur;
            n += 1;
        }
        n
    }

    /// `scan` plus instrumentation.
    fn scan_timed(&self, head: &Node, max: usize, rec: bool) {
        let hs = head.head_state();
        if !rec {
            // SAFETY: called by the head on its own state.
            unsafe { Self::scan(head, hs, max) };
            return;
        }
        let t0 = cycles();
        // SAFETY: as above.
        let n = unsafe { Self::scan(head, hs, max) };
        if n == 0 {
            return;
        }
        let t1 = cycles();
        let st = head.stats();
        st.scan_cycles += t1.wrapping_sub(t0);
        st.scan_visited += n;
        if self.word.word.load(Relaxed) & LOCKED == 0 {
            let u = self.acct.last_unlock.load(Relaxed);
            if u < t1 {
                st.scan_on_path_cycles += t1 - t0.max(u);
            }
        }
    }

    /// Owner only: fold a finished request's charge into the running totals.
    #[inline]
    fn account(&self, cs: u64) {
        let a = &*self.acct;
        a.usage_total
            .store(a.usage_total.load(Relaxed).wrapping_add(cs), Relaxed);
        a.served_total
            .store(a.served_total.load(Relaxed) + 1, Relaxed);
    }

    /// Owner only: clear `LOCKED`; wake a parked head. Returns whether it
    /// woke one.
    #[inline]
    fn release(&self, t_end: u64) -> bool {
        if self.opts.record_handoffs {
            self.acct.last_cs_end.store(t_end, Relaxed);
            self.acct.last_unlock.store(cycles(), Relaxed);
        }
        // SeqCst: Dekker with the head's `parked_head` store + word load.
        self.word.word.fetch_and(!LOCKED, SeqCst);
        if self.word.parked_head.load(SeqCst).is_null() {
            return false;
        }
        let p = self.word.parked_head.swap(ptr::null_mut(), AcqRel);
        if p.is_null() {
            return false;
        }
        // SAFETY: nodes live as long as the lock.
        unsafe { &*p }.signal.waker.wake(self.opts.wake_placement);
        true
    }

    /// Instrumentation snapshot of the previous owner's timestamps.
    #[inline]
    fn timing(&self, t_acq: u64, head: Option<(HeadAcq, u64)>) -> Option<Timing> {
        self.opts.record_handoffs.then(|| Timing {
            t_acq,
            prev_end: self.acct.last_cs_end.load(Relaxed),
            prev_unlock: self.acct.last_unlock.load(Relaxed),
            head,
        })
    }
}

impl<T: Send + 'static> DelegationLock<T> for Cfl<T> {
    type Client = CflClient<T>;

    fn new(data: T) -> Self {
        Self::with_options(data, CflOptions::default())
    }

    fn client(self: &Arc<Self>) -> CflClient<T> {
        // SAFETY: `Box::into_raw` never returns null.
        let node = unsafe { NonNull::new_unchecked(Box::into_raw(Box::new(Node::new()))) };
        self.nodes.lock().push(node);
        CflClient {
            lock: Arc::clone(self),
            node,
        }
    }

    fn name() -> &'static str {
        "cfl"
    }
}

// ---------------------------------------------------------------------------
// Client and the `run` future
// ---------------------------------------------------------------------------

pub struct CflClient<T> {
    lock: Arc<Cfl<T>>,
    /// Lock-owned node (see module docs, "Node lifetime").
    node: NonNull<Node>,
}

// SAFETY: the node is lock-owned; other threads reach it only through the
// queue protocol.
unsafe impl<T: Send> Send for CflClient<T> {}

impl<T> CflClient<T> {
    #[inline]
    fn node(&self) -> &Node {
        // SAFETY: the lock (kept alive by `self.lock`) owns the node.
        unsafe { self.node.as_ref() }
    }

    /// Test only: set this client's charged usage (counts as one service).
    #[cfg(test)]
    fn precharge(&self, usage: u64) {
        self.node().charge(usage);
    }
}

impl<T> Drop for CflClient<T> {
    fn drop(&mut self) {
        if self.lock.opts.record_handoffs {
            let s = *self.node().stats();
            if s != CflStats::default() {
                self.lock.stats.lock().add(&s);
            }
        }
    }
}

impl<T: Send + 'static> LockClient<T> for CflClient<T> {
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
        self.node().usage()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Init,
    /// Queued behind a head, waiting for headship.
    Waiting,
    /// Queue head: next owner, waiting for the lock word.
    Head,
    Done,
}

struct Run<'a, T, F> {
    client: &'a mut CflClient<T>,
    f: Option<F>,
    state: State,
}

impl<T, F> Unpin for Run<'_, T, F> {}

impl<T, R, F> Run<'_, T, F>
where
    F: FnOnce(&mut T) -> R,
{
    fn poll_waiting(&mut self, cx: &Context<'_>) -> Poll<R> {
        let lock = &*self.client.lock;
        let node = self.client.node();
        if node.signal.status.load(Acquire) != HEAD {
            node.signal
                .waker
                .register(cx.waker(), lock.opts.wake_placement);
            if node.signal.status.load(Acquire) != HEAD {
                return Poll::Pending;
            }
        }
        self.state = State::Head;
        self.poll_head(cx)
    }

    fn poll_head(&mut self, cx: &Context<'_>) -> Poll<R> {
        let lock = &*self.client.lock;
        let node = self.client.node();
        let me = node as *const Node as *mut Node;
        let rec = lock.recording();
        let word = &lock.word.word;
        let usage = lock.opts.shuffle == Shuffle::Usage;
        let full = lock.opts.scan == Scan::Full;
        let budget = lock.opts.head_spin_cycles;
        {
            let hs = node.head_state();
            if hs.first_poll {
                hs.first_poll = false;
                let g = node.signal.grant_tsc.load(Relaxed);
                if rec && g != 0 {
                    let st = node.stats();
                    st.grants += 1;
                    st.grant_to_poll_cycles += cycles().saturating_sub(g);
                    if word.load(Relaxed) & LOCKED != 0 {
                        st.early_heads += 1;
                    }
                }
            }
        }
        let mut saw_held = false;
        loop {
            if usage && full {
                lock.scan_timed(node, usize::MAX, rec);
            }
            let t_spin = cycles();
            loop {
                let w = word.load(Relaxed);
                if w & LOCKED == 0 {
                    if word
                        .compare_exchange_weak(w, w | LOCKED, Acquire, Relaxed)
                        .is_ok()
                    {
                        let t_acq = cycles();
                        let how = if saw_held {
                            HeadAcq::Spin
                        } else if node.head_state().parked {
                            HeadAcq::Woken
                        } else {
                            HeadAcq::Late
                        };
                        if rec {
                            node.stats().spin_cycles += t_acq.wrapping_sub(t_spin);
                        }
                        return Poll::Ready(self.acquired(t_acq, how));
                    }
                    continue;
                }
                saw_held = true;
                if w & NO_STEAL == 0 {
                    // Re-assert after a late `fetch_and` of a head that left
                    // an empty queue (module docs; steals are not bounded).
                    word.fetch_or(NO_STEAL, Relaxed);
                }
                if usage {
                    lock.scan_timed(node, SCAN_CHUNK, rec);
                }
                if cycles().wrapping_sub(t_spin) >= budget {
                    break;
                }
                std::hint::spin_loop();
            }
            if rec {
                node.stats().spin_cycles += cycles().wrapping_sub(t_spin);
            }
            node.signal
                .waker
                .register(cx.waker(), lock.opts.wake_placement);
            lock.word.parked_head.store(me, SeqCst);
            if word.load(SeqCst) & LOCKED != 0 {
                node.head_state().parked = true;
                if rec {
                    node.stats().parks += 1;
                }
                return Poll::Pending;
            }
            // Released between the last check and the publication: withdraw
            // (a releaser that already took the entry sends a spurious wake).
            let _ = lock
                .word
                .parked_head
                .compare_exchange(me, ptr::null_mut(), AcqRel, Relaxed);
            if rec {
                node.stats().park_rechecks += 1;
            }
        }
    }

    /// The head holds the lock word: pick and splice its successor, pass
    /// headship, run the critical section.
    fn acquired(&mut self, t_acq: u64, how: HeadAcq) -> R {
        let lock = &*self.client.lock;
        let node = self.client.node();
        let me = node as *const Node as *mut Node;
        if lock.word.parked_head.load(Relaxed) == me {
            let _ = lock
                .word
                .parked_head
                .compare_exchange(me, ptr::null_mut(), Relaxed, Relaxed);
        }
        let tick = lock.acct.handoffs.load(Relaxed) + 1;
        lock.acct.handoffs.store(tick, Relaxed);
        let timing = lock.timing(t_acq, Some((how, tick)));
        let rec = lock.recording();

        let hs = node.head_state();
        let first = node.link.next.load(Acquire);
        let mut succ = first;
        if !first.is_null() && lock.opts.shuffle == Shuffle::Usage {
            let clamp = lock.opts.starvation_clamp;
            // SAFETY: queued node, lock-owned.
            let f = unsafe { &*first };
            if clamp != 0 && tick.wrapping_sub(f.link.enq_tick.load(Relaxed)) > clamp {
                if rec {
                    node.stats().promoted += 1;
                }
            } else if !hs.best.is_null() && hs.best != first {
                // SAFETY: we are the head. `best` was movable when examined
                // (its `next` was set). Only the head writes interior links,
                // and no node leaves the queue except through headship (no
                // cancellation after enqueue, see `Drop for Run`). So
                // `best_prev.next == best` still holds, and `best.next` is
                // still that non-null successor.
                unsafe {
                    let b = &*hs.best;
                    let bn = b.link.next.load(Relaxed);
                    (&*hs.best_prev).link.next.store(bn, Relaxed);
                    b.link.next.store(first, Relaxed);
                    node.link.next.store(hs.best as *mut Node, Relaxed);
                }
                succ = hs.best as *mut Node;
                if rec {
                    node.stats().moves += 1;
                }
            }
        }
        if succ.is_null() {
            if lock
                .tail
                .compare_exchange(me, ptr::null_mut(), AcqRel, Acquire)
                .is_ok()
            {
                // Queue empty: allow the uncontended path again.
                lock.word.word.fetch_and(!NO_STEAL, Relaxed);
            } else {
                // An enqueuer swapped the tail and is about to link us.
                loop {
                    succ = node.link.next.load(Acquire);
                    if !succ.is_null() {
                        break;
                    }
                    std::hint::spin_loop();
                }
            }
        }
        if !succ.is_null() {
            // SAFETY: queued node, lock-owned.
            let s = unsafe { &*succ };
            s.signal.grant_tsc.store(
                if lock.opts.record_handoffs {
                    cycles()
                } else {
                    0
                },
                Relaxed,
            );
            s.signal.status.store(HEAD, Release);
            if lock.opts.prewake {
                s.signal.waker.wake(lock.opts.wake_placement);
            } else {
                hs.successor = succ;
            }
        }
        let base = node.link.key.load(Relaxed);
        self.critical(base, timing)
    }

    /// Owner: run the critical section, charge it, release.
    fn critical(&mut self, base: u64, timing: Option<Timing>) -> R {
        let f = self.f.take().expect("closure taken twice");
        let lock = &*self.client.lock;
        let node = self.client.node();
        let t0 = cycles();
        // SAFETY: we hold the lock word.
        let r = f(unsafe { &mut *lock.data.get() });
        let t1 = cycles();
        let cs = t1.wrapping_sub(t0);
        node.charge(base.wrapping_add(cs));
        lock.account(cs);
        self.state = State::Done;
        let woke = lock.release(t1);
        let hs = node.head_state();
        if !hs.successor.is_null() {
            // SAFETY: lock-owned node.
            let s = unsafe { &*hs.successor };
            hs.successor = ptr::null();
            s.signal.waker.wake(lock.opts.wake_placement);
        }
        if let Some(t) = timing.filter(|_| lock.recording()) {
            let st = node.stats();
            st.head_wakes += woke as u64;
            match t.head {
                None => st.fast += 1,
                Some((how, tick)) => {
                    st.record_wait(tick.wrapping_sub(node.link.enq_tick.load(Relaxed)));
                    if t.prev_unlock != 0 && t.prev_end <= t.prev_unlock && t.prev_unlock <= t.t_acq
                    {
                        let react = t.t_acq - t.prev_unlock;
                        st.handoffs += 1;
                        st.release_cycles += t.prev_unlock - t.prev_end;
                        st.react_cycles += react;
                        st.pass_cycles += t0.wrapping_sub(t.t_acq);
                        let (n, c) = match how {
                            HeadAcq::Spin => (&mut st.acq_spin, &mut st.acq_spin_react),
                            HeadAcq::Late => (&mut st.acq_late, &mut st.acq_late_react),
                            HeadAcq::Woken => (&mut st.acq_woken, &mut st.acq_woken_react),
                        };
                        *n += 1;
                        *c += react;
                    }
                }
            }
        } else if woke && lock.recording() {
            node.stats().head_wakes += 1;
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
        match this.state {
            State::Init => {
                let lock = &*this.client.lock;
                if lock.try_fast() {
                    let timing = lock.timing(cycles(), None);
                    let base = this.client.node().usage();
                    return Poll::Ready(this.critical(base, timing));
                }
                if lock.enqueue(this.client.node()) {
                    this.state = State::Head;
                    this.poll_head(cx)
                } else {
                    this.state = State::Waiting;
                    this.poll_waiting(cx)
                }
            }
            State::Waiting => this.poll_waiting(cx),
            State::Head => this.poll_head(cx),
            State::Done => panic!("cfl::Run polled after completion"),
        }
    }
}

impl<T, F> Drop for Run<'_, T, F> {
    fn drop(&mut self) {
        if matches!(self.state, State::Waiting | State::Head) && !std::thread::panicking() {
            eprintln!(
                "cfl: a queued request was dropped; cancellation after enqueue is unsupported"
            );
            std::process::abort();
        }
    }
}

// ---------------------------------------------------------------------------
// Test hooks
// ---------------------------------------------------------------------------

#[cfg(test)]
impl<T> Cfl<T> {
    /// Take the lock from outside a `run` future (uncontended path only).
    fn hold(&self) -> bool {
        self.try_fast()
    }

    /// Release a lock taken by `hold`, waking a parked head.
    fn release_held(&self) {
        self.release(cycles());
    }

    /// The protected value; caller guarantees nobody owns the lock.
    fn data(&self) -> &T {
        // SAFETY: see above.
        unsafe { &*self.data.get() }
    }

    /// Stats folded in from dropped clients.
    fn handoff_stats(&self) -> CflStats {
        *self.stats.lock()
    }
}

#[cfg(test)]
mod tests {
    use super::super::fc::testing::{run_threads, spin_cycles, BoxFut};
    use super::*;
    use crate::executor::Executor;
    use crate::stats::TaskKind;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::task::Waker;
    use std::time::Duration;

    /// Deadline for one real-executor run; a lost wakeup hangs it.
    const DEADLINE: Duration = Duration::from_secs(60);
    /// Test-executor stall limit: nothing runnable this long = lost wakeup.
    const STALL: Duration = Duration::from_secs(5);

    fn opts(shuffle: Shuffle, prewake: bool, clamp: u64) -> CflOptions {
        CflOptions {
            shuffle,
            prewake,
            starvation_clamp: clamp,
            ..CflOptions::default()
        }
    }

    /// Hold the lock, queue one request per client (client i precharged to
    /// `usages[i]`, queued in index order) whose critical section logs i,
    /// release, then poll round-robin with a no-op waker until all are done.
    /// Heads never spin, so every poll that finds the lock held parks.
    /// Returns the service order.
    fn service_log(o: CflOptions, usages: &[u64]) -> Vec<usize> {
        let o = CflOptions {
            head_spin_cycles: 0,
            ..o
        };
        let lock = Arc::new(Cfl::with_options(Vec::<usize>::new(), o));
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
            assert!(left < before, "no queued request made progress");
        }
        drop(runs);
        drop(clients);
        let log = lock.data().clone();
        assert!(
            lock.hold(),
            "lock left held (or NO_STEAL set) after the last handoff"
        );
        log
    }

    /// Client 0 is head (first to queue); every later head picks the
    /// cheapest movable waiter behind it. The last client (sentinel, huge
    /// usage) stays the tail, so every other waiter is movable.
    #[test]
    fn successor_is_the_min_usage_waiter_behind_the_head() {
        let usages = [0, 300, 100, 400, 200, 1 << 40];
        for o in [CflOptions::default(), opts(Shuffle::Usage, false, 0)] {
            assert_eq!(service_log(o, &usages), vec![0, 2, 4, 1, 3, 5]);
        }
        let o = CflOptions {
            scan: Scan::Overlap,
            ..CflOptions::default()
        };
        // Overlap scans only while the word is held. Here every head is
        // polled after its predecessor has released (each critical section
        // completes inside one poll), so no head ever scans: FIFO. Usage
        // order under Overlap needs heads that arrive during the critical
        // section.
        assert_eq!(service_log(o, &usages), vec![0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn shuffle_off_is_fifo() {
        let usages = [0, 300, 100, 400, 200, 1 << 40];
        for prewake in [true, false] {
            assert_eq!(
                service_log(opts(Shuffle::Off, prewake, 0), &usages),
                vec![0, 1, 2, 3, 4, 5]
            );
        }
    }

    /// Client 1 (usage 1e6) is the oldest waiter behind head 0, cheap
    /// clients after it. Handoff t (the t-th acquisition by a head) passes
    /// headship to the oldest waiter once t − its enqueue tick (0) exceeds
    /// the clamp: the clamp counts handoffs.
    #[test]
    fn starvation_clamp_counts_handoffs() {
        let usages = [0, 1_000_000, 10, 20, 30, 40, 1 << 40];
        let log = |clamp| service_log(opts(Shuffle::Usage, true, clamp), &usages);
        assert_eq!(log(0), vec![0, 2, 3, 4, 5, 1, 6]);
        assert_eq!(log(1), vec![0, 2, 1, 3, 4, 5, 6]);
        assert_eq!(log(2), vec![0, 2, 3, 1, 4, 5, 6]);
    }

    /// `threads` OS threads x `per_thread` clients x `ops` tiny critical
    /// sections with random gaps of up to `max_gap` cycles, on executors
    /// that poll only woken tasks: a head stranded while parked stalls its
    /// executor (panic after `STALL`). Heads never spin, so every head that
    /// finds the word held parks. Returns the lock's stats.
    fn race(prewake: bool, threads: usize, per_thread: usize, ops: u64, max_gap: u64) -> CflStats {
        let o = CflOptions {
            prewake,
            head_spin_cycles: 0,
            starvation_clamp: 1,
            record_handoffs: true,
            ..CflOptions::default()
        };
        let lock = Arc::new(Cfl::with_options(0u64, o));
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
        assert_eq!(s.fast + s.queued(), total, "{s:?}");
        s
    }

    /// Both sides of the park/release race must occur: heads that parked
    /// and were woken by a releaser, and park attempts whose SeqCst re-check
    /// found the word already cleared. No request may be stranded.
    #[test]
    fn no_lost_wakeup_across_head_park_and_release() {
        // Stats count only while recording; no other test reads them.
        stats::set_recording(true);
        for prewake in [true, false] {
            let dense = race(prewake, 4, 3, 20_000, 2_000);
            let sparse = race(prewake, 4, 1, 20_000, 20_000);
            let parks = dense.parks + sparse.parks;
            let wakes = dense.head_wakes + sparse.head_wakes;
            let rechecks = dense.park_rechecks + sparse.park_rechecks;
            assert!(
                parks > 0 && wakes > 0 && rechecks > 0,
                "prewake={prewake}: parks {parks} wakes {wakes} rechecks {rechecks}\n{dense:?}\n{sparse:?}"
            );
        }
    }

    /// Two cost classes (8:1) on 4 OS threads under pure usage order (clamp
    /// off): cheap clients get more requests served and the charged usage
    /// evens out; FIFO would give equal request counts.
    #[test]
    fn cheaper_clients_are_served_more() {
        const LIGHT: u64 = 2_000;
        const HEAVY: u64 = 8 * LIGHT;
        for prewake in [true, false] {
            let lock = Arc::new(Cfl::with_options(0u64, opts(Shuffle::Usage, prewake, 0)));
            let stop = Arc::new(AtomicBool::new(false));
            let served = Arc::new(AtomicU64::new(0));
            let out = Arc::new(Mutex::new(Vec::new()));
            let tasks: Vec<Vec<BoxFut>> = (0..4)
                .map(|_| {
                    [LIGHT, HEAVY, LIGHT, HEAVY]
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
                "prewake={prewake}: light {light_ops} vs heavy {heavy_ops} ops: {out:?}"
            );
            assert!(
                (0.5..=2.0).contains(&usage_ratio),
                "prewake={prewake}: heavy/light charged usage {usage_ratio:.2}: {out:?}"
            );
        }
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
    /// odd clients' critical sections `heavy`x longer, random gaps of up to
    /// `max_gap` cycles. Returns (count, overlaps, per-client usage, stats).
    fn stress(
        o: CflOptions,
        workers: usize,
        clients: usize,
        ops: u64,
        cs: u64,
        heavy: u64,
        max_gap: u64,
    ) -> (u64, u64, Vec<u64>, CflStats) {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let lock = Arc::new(Cfl::with_options(Guarded::default(), o));
            let exec = Executor::with_balance_interval(workers, 31);
            let tasks: Vec<_> = (0..clients)
                .map(|i| {
                    let lock = Arc::clone(&lock);
                    let cs = if i % 2 == 1 { cs * heavy } else { cs };
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
            let s = lock.handoff_stats();
            let _ = tx.send((count, overlaps, usage, s));
        });
        match rx.recv_timeout(DEADLINE) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => {
                panic!("run did not finish within {DEADLINE:?}: lost wakeup")
            }
            Err(RecvTimeoutError::Disconnected) => panic!("run panicked (see above)"),
        }
    }

    /// Every variant: shuffle {usage, off} x pre-wake {on, off} x scan
    /// {full, overlap} (usage only) x placement {home, remote, default} x
    /// head spin {default, 0 = always park}; sustained (no gaps) and sparse
    /// (gaps up to 40 k cycles: the queue drains and the uncontended path
    /// mixes with handoffs).
    #[test]
    fn mutual_exclusion_and_completion_on_the_executor() {
        let cs = 200;
        let mut variants = Vec::new();
        for (shuffle, scan) in [
            (Shuffle::Usage, Scan::Full),
            (Shuffle::Usage, Scan::Overlap),
            (Shuffle::Off, Scan::Full),
        ] {
            for prewake in [true, false] {
                for wake_placement in [
                    WakePlacement::Home,
                    WakePlacement::Remote,
                    WakePlacement::Default,
                ] {
                    for head_spin_cycles in [DEFAULT_HEAD_SPIN_CYCLES, 0] {
                        variants.push(CflOptions {
                            shuffle,
                            scan,
                            prewake,
                            wake_placement,
                            head_spin_cycles,
                            ..CflOptions::default()
                        });
                    }
                }
            }
        }
        for o in variants {
            for (clients, ops, max_gap) in [(16, 1_500, 0), (8, 1_000, 40_000)] {
                let name = format!("{o:?} gap={max_gap}");
                let (count, overlaps, usage, _) = stress(o, 4, clients, ops, cs, 1, max_gap);
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

    /// Clamp c under a 1:50 cost mix on the real executor: every queued
    /// request waits at most c + 2 handoffs plus one per client (older
    /// starving waiters are served first), and the clamp actually binds.
    #[test]
    fn starvation_bound_holds_under_contention() {
        stats::set_recording(true);
        let clients = 16;
        for clamp in [4, 16] {
            let o = CflOptions {
                starvation_clamp: clamp,
                record_handoffs: true,
                ..CflOptions::default()
            };
            let (count, overlaps, _, s) = stress(o, 4, clients, 1_000, 200, 50, 0);
            assert_eq!(overlaps, 0);
            assert_eq!(count, clients as u64 * 1_000);
            assert!(s.queued() > 0 && s.promoted > 0, "clamp {clamp}: {s:?}");
            assert!(
                s.max_wait <= clamp + 2 + clients as u64,
                "clamp {clamp}: max wait {} handoffs",
                s.max_wait
            );
        }
        let _ = Ordering::Relaxed;
    }
}
