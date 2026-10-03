//! Work-stealing executor with an explicit task-placement hint.
//!
//! Structure (see `RESEARCH.md`, "Executor contract"):
//! - `workers` OS threads, each pinned to a distinct physical core, each owning
//!   a `crossbeam_deque::Worker` (FIFO) plus a single **run-next slot**; one
//!   shared `Injector`; idle workers park (`parking`).
//! - Tasks are `async_task` tasks whose metadata is a [`TaskMeta`]
//!   (`TaskKind`, schedule timestamp, inline-resume flag).
//!
//! ## Placement hint
//!
//! A `Waker` can only enqueue a task; it carries no placement information.
//! Locks that need CES-style placement therefore set a **thread-local
//! placement hint** immediately before waking, and the schedule callback
//! consumes it (`take`, resetting to `Default`):
//!
//! - [`wake_inline`]: `Inline` + `wake_by_ref`. If the target task is not
//!   running, `async_task` invokes the schedule callback synchronously on this
//!   thread, which puts the task in this worker's run-next slot and sets the
//!   task's `inline_resumed` flag. The hint is reset afterwards so a deferred
//!   schedule (target task currently running elsewhere) cannot leak the hint
//!   onto an unrelated wake.
//! - [`reschedule_self_remote`]: `Remote` + `wake_by_ref` on the *current*
//!   task. Because the task is running, `async_task` defers the schedule
//!   callback until the poll returns `Pending`, where it runs on this same
//!   thread and finds the `Remote` hint: the task goes to the injector and one
//!   idle worker is unparked. Invariant relied upon: the future returns
//!   `Pending` immediately after calling this, with no other schedule
//!   callback in between on this thread.
//! - `Default`: local queue when on a worker, else the injector.
//!
//! The run-next slot is checked first on every loop iteration, so an inline
//! task runs right after the current poll returns, before the local queue and
//! before stealing. There is deliberately **no** anti-starvation cap on
//! consecutive run-next executions: an unbounded CES combining chain on one
//! worker is exactly the effect under study (F1/F2). The slot is a single
//! cell; a second inline schedule within one poll displaces the previous
//! occupant to the back of the local queue (does not happen with one lock).
//!
//! Every `INJECTOR_CHECK_INTERVAL` polls a worker drains up to `len/W + 1`
//! injector tasks (tokio's share rule), polling each immediately rather than
//! parking a batch in its local queue, so remotely rescheduled tasks cannot
//! starve behind a self-yielding local task and no batch can be trapped
//! behind a run-next chain (see `steal_injector`).
//! Every `balance_interval` polls (default
//! `DEFAULT_BALANCE_INTERVAL`, 0 = off) a busy worker additionally moves half
//! of a random peer's local queue to the back of its own: plain
//! steal-when-idle never fires when each worker hosts a self-yielding
//! bystander, and then every `Default`-placed wake converges on the waking
//! worker (see `Executor::with_balance_interval`).
//!
//! ## Parking protocol (no lost wake-ups)
//!
//! Worker: push own id onto `idle` (mutex), `idle_count += 1` (SeqCst RMW),
//! `fence(SeqCst)`, re-check injector and all stealers, then park.
//! Producer: push runnable, `fence(SeqCst)`, load `idle_count`; if non-zero pop
//! one id and `unpark` it. The two SeqCst fences forbid the store-buffering
//! outcome, so either the worker sees the push or the producer sees the
//! registration. `parking`'s token is sticky, so an unpark that arrives before
//! `park()` returns immediately (a harmless spurious iteration).

use std::cell::{Cell, OnceCell};
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{fence, AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::JoinHandle;

use async_task::{Builder, ScheduleInfo, WithInfo};
use crossbeam_deque::{Injector, Steal, Stealer, Worker};
use parking::{Parker, Unparker};
use parking_lot::Mutex;

use crate::lock::cycles;
use crate::stats::{self, Histogram, TaskKind, WorkerStats, MAX_WORKERS};

/// Polls between forced injector checks (tokio uses 61; polls here are short).
pub const INJECTOR_CHECK_INTERVAL: u32 = 31;

/// Default polls between balancing steals (`Executor::new`); see
/// [`Executor::with_balance_interval`].
pub const DEFAULT_BALANCE_INTERVAL: u32 = 31;

/// Origin marker for tasks scheduled from a non-worker thread.
const NO_WORKER: usize = usize::MAX;

/// Metadata stored inside every task allocation.
pub struct TaskMeta {
    pub kind: TaskKind,
    /// `cycles()` at the last schedule (written before the runnable is
    /// pushed; the deque's Release/Acquire publishes it to the poller).
    scheduled_at: AtomicU64,
    /// Worker whose queue the task was last scheduled onto (`NO_WORKER` for
    /// the injector from off-worker). Bystander latency is attributed to it.
    scheduled_on: AtomicUsize,
    /// Worker that last polled the task (`NO_WORKER` before the first poll);
    /// the *home* of [`wake_home`].
    last_worker: AtomicUsize,
    /// Set when the task was placed in a run-next slot; consumed (swapped to
    /// false) by the worker at the start of the next poll.
    inline_resumed: AtomicBool,
}

impl TaskMeta {
    fn new(kind: TaskKind) -> Self {
        TaskMeta {
            kind,
            scheduled_at: AtomicU64::new(0),
            scheduled_on: AtomicUsize::new(NO_WORKER),
            last_worker: AtomicUsize::new(NO_WORKER),
            inline_resumed: AtomicBool::new(false),
        }
    }
}

pub type Runnable = async_task::Runnable<TaskMeta>;
pub type Task<R> = async_task::Task<R, TaskMeta>;

/// Where the next scheduled task on this thread should go.
///
/// - `Default`: local queue when scheduled from a worker, else the injector.
/// - `Inline`: this worker's run-next slot (CES inline resume).
/// - `Remote`: injector + unpark one idle worker.
/// - `Home`: the inbox of the worker that last polled the task; falls back
///   to `Default` if the task has never been polled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Placement {
    #[default]
    Default,
    Inline,
    Remote,
    Home,
}

impl Placement {
    pub const COUNT: usize = 4;

    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            Placement::Default => "default",
            Placement::Inline => "inline",
            Placement::Remote => "remote",
            Placement::Home => "home",
        }
    }
}

thread_local! {
    static PLACEMENT_HINT: Cell<Placement> = const { Cell::new(Placement::Default) };
    static CTX: OnceCell<WorkerCtx> = const { OnceCell::new() };
}

struct WorkerCtx {
    id: usize,
    local: Worker<Runnable>,
    run_next: Cell<Option<Runnable>>,
    /// Whether the task currently being polled was inline-resumed here.
    current_inline: Cell<bool>,
    /// Consecutive inline-resumed polls including the current one.
    chain_len: Cell<u32>,
    /// `cycles()` at the first poll of the current chain.
    chain_start: Cell<u64>,
    /// Schedules issued from this worker, by placement (Default = local).
    placements: [Cell<u64>; Placement::COUNT],
    shared: Arc<Shared>,
}

struct Shared {
    injector: Injector<Runnable>,
    stealers: Vec<Stealer<Runnable>>,
    /// Per-worker inbox for `Executor::spawn_on`: drained by the owning
    /// worker into its local queue at the top of every loop iteration.
    inboxes: Vec<Injector<Runnable>>,
    unparkers: Vec<Unparker>,
    /// Stack of parked (or about-to-park) worker ids.
    idle: Mutex<Vec<usize>>,
    idle_count: AtomicUsize,
    shutdown: AtomicBool,
    /// 0 disables balancing steals.
    balance_interval: u32,
}

impl Shared {
    /// Called after a push; wakes one idle worker if any.
    #[inline]
    fn notify(&self) {
        fence(Ordering::SeqCst);
        if self.idle_count.load(Ordering::Relaxed) == 0 {
            return;
        }
        let id = self.idle.lock().pop();
        if let Some(id) = id {
            self.idle_count.fetch_sub(1, Ordering::SeqCst);
            self.unparkers[id].unpark();
        }
    }

    /// Called after a push to `inboxes[id]`: wake that worker. The idle list
    /// is only touched when someone is idle; otherwise the sticky unpark
    /// token merely makes the target's next `park()` return at once.
    fn unpark_worker(&self, id: usize) {
        fence(Ordering::SeqCst);
        if self.idle_count.load(Ordering::Relaxed) != 0 {
            let mut idle = self.idle.lock();
            if let Some(pos) = idle.iter().position(|&x| x == id) {
                idle.swap_remove(pos);
                self.idle_count.fetch_sub(1, Ordering::SeqCst);
            }
        }
        self.unparkers[id].unpark();
    }

    fn has_work(&self, me: usize) -> bool {
        !self.injector.is_empty()
            || !self.inboxes[me].is_empty()
            || self.stealers.iter().any(|s| !s.is_empty())
    }

    fn push_remote(&self, runnable: Runnable) {
        self.injector.push(runnable);
        self.notify();
    }
}

// ---------------------------------------------------------------------------
// Public thread-level API
// ---------------------------------------------------------------------------

/// Index of the executor worker running on this thread, `None` elsewhere.
#[inline]
pub fn worker_id() -> Option<usize> {
    CTX.with(|c| c.get().map(|c| c.id))
}

/// True if the task currently being polled on this worker was placed via
/// the inline slot (used by `ces` to attribute critical-section cycles to
/// the combining worker).
#[inline]
pub fn current_task_inline_resumed() -> bool {
    CTX.with(|c| c.get().is_some_and(|c| c.current_inline.get()))
}

/// Set the placement hint consumed by the next schedule callback on this
/// thread. Prefer [`wake_inline`] / [`reschedule_self_remote`].
#[inline]
pub fn set_placement_hint(p: Placement) {
    PLACEMENT_HINT.set(p);
}

/// Wake `waker`'s task so that it runs next on this worker (run-next slot).
/// Falls back to the injector when called off-worker or when the target task
/// is currently running elsewhere (its deferred schedule sees `Default`).
#[inline]
pub fn wake_inline(waker: &Waker) {
    wake_with(Placement::Inline, waker);
}

/// Wake `waker`'s task through the injector, unparking one idle worker.
#[inline]
pub fn wake_remote(waker: &Waker) {
    wake_with(Placement::Remote, waker);
}

/// Wake `waker`'s task onto its *home* worker (the worker that last polled
/// it) via that worker's inbox, unparking it if idle. Falls back to
/// `Default` placement when the task has no home yet. Off-executor callers
/// still reach the inbox because the schedule callback owns the executor
/// handle. No allocation per call; safe in a combiner loop.
#[inline]
pub fn wake_home(waker: &Waker) {
    wake_with(Placement::Home, waker);
}

/// Wake with an explicit placement (the hint is set for the duration of the
/// `wake_by_ref` call only; a deferred schedule of a task that is currently
/// running elsewhere sees `Default` on its own thread).
#[inline]
pub fn wake_with(placement: Placement, waker: &Waker) {
    PLACEMENT_HINT.set(placement);
    waker.wake_by_ref();
    PLACEMENT_HINT.set(Placement::Default);
}

/// Arrange for the *current* task to be rescheduled through the injector
/// (with one other worker unparked) once the current poll returns `Pending`.
/// The caller MUST return `Pending` right away.
#[inline]
pub fn reschedule_self_remote(cx: &Context<'_>) {
    PLACEMENT_HINT.set(Placement::Remote);
    cx.waker().wake_by_ref();
}

/// Consecutive inline-resumed polls on this worker up to and including the
/// current one (`0` if the current task was not inline-resumed), and the
/// `cycles()` timestamp at which the chain's first poll started. Locks use
/// it to bound a CES chain.
#[inline]
pub fn current_chain() -> (u32, u64) {
    CTX.with(|c| {
        c.get()
            .map_or((0, 0), |c| (c.chain_len.get(), c.chain_start.get()))
    })
}

/// Run `runnable` on this worker before anything else, right after the
/// current poll returns. Worker-only; off-worker it goes to the injector of
/// no executor and therefore panics.
pub fn schedule_inline(runnable: Runnable) {
    CTX.with(|c| match c.get() {
        Some(ctx) => ctx.put_run_next(runnable),
        None => panic!("schedule_inline called off-worker"),
    });
}

/// Push `runnable` to the injector and unpark one other worker. Worker-only.
pub fn schedule_remote(runnable: Runnable) {
    CTX.with(|c| match c.get() {
        Some(ctx) => ctx.shared.push_remote(runnable),
        None => panic!("schedule_remote called off-worker"),
    });
}

/// Spawn `fut` onto the executor that owns the current worker thread, from
/// inside a task: the first schedule goes through the normal schedule
/// callback (placement hint as set, `Default` = this worker's local queue),
/// exactly like [`Executor::spawn`] called on a worker. `None` off-worker.
///
/// Hook for locks that own a service task (`locks::actor` starts its server
/// lazily from the first request); it adds no scheduling policy.
pub fn spawn_here<F>(kind: TaskKind, fut: F) -> Option<Task<F::Output>>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let shared = CTX.with(|c| c.get().map(|ctx| Arc::clone(&ctx.shared)))?;
    let (runnable, task) = build_task(&shared, kind, fut);
    runnable.schedule();
    Some(task)
}

fn build_task<F>(shared: &Arc<Shared>, kind: TaskKind, fut: F) -> (Runnable, Task<F::Output>)
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let shared = Arc::clone(shared);
    Builder::new()
        .metadata(TaskMeta::new(kind))
        .propagate_panic(true)
        .spawn(
            move |_| fut,
            WithInfo(move |r: Runnable, info: ScheduleInfo| schedule(&shared, r, info)),
        )
}

impl WorkerCtx {
    fn put_run_next(&self, runnable: Runnable) {
        let meta = runnable.metadata();
        meta.inline_resumed.store(true, Ordering::Relaxed);
        meta.scheduled_on.store(self.id, Ordering::Relaxed);
        if let Some(prev) = self.run_next.replace(Some(runnable)) {
            prev.metadata()
                .inline_resumed
                .store(false, Ordering::Relaxed);
            self.local.push(prev);
            self.shared.notify();
        }
    }

    fn push_local(&self, runnable: Runnable) {
        runnable
            .metadata()
            .scheduled_on
            .store(self.id, Ordering::Relaxed);
        self.local.push(runnable);
        self.shared.notify();
    }

    /// Whether the run-next slot is occupied (without consuming it).
    fn run_next_pending(&self) -> bool {
        let r = self.run_next.take();
        let some = r.is_some();
        self.run_next.set(r);
        some
    }
}

/// Deliver `runnable` to the inbox of `home` and wake that worker.
fn push_home(shared: &Shared, home: usize, runnable: Runnable) {
    runnable
        .metadata()
        .scheduled_on
        .store(home, Ordering::Relaxed);
    shared.inboxes[home].push(runnable);
    shared.unpark_worker(home);
}

/// Schedule callback installed on every task.
fn schedule(shared: &Arc<Shared>, runnable: Runnable, _info: ScheduleInfo) {
    let meta = runnable.metadata();
    meta.scheduled_at.store(cycles(), Ordering::Relaxed);
    meta.scheduled_on.store(NO_WORKER, Ordering::Relaxed);
    let mut hint = PLACEMENT_HINT.replace(Placement::Default);
    let home = meta.last_worker.load(Ordering::Relaxed);
    if hint == Placement::Home && home >= shared.stealers.len() {
        hint = Placement::Default; // never polled: no home yet
    }
    CTX.with(|c| match c.get() {
        Some(ctx) if Arc::ptr_eq(&ctx.shared, shared) => {
            ctx.placements[hint.index()].set(ctx.placements[hint.index()].get() + 1);
            match hint {
                Placement::Inline => ctx.put_run_next(runnable),
                Placement::Remote => shared.push_remote(runnable),
                Placement::Home => push_home(shared, home, runnable),
                Placement::Default => ctx.push_local(runnable),
            }
        }
        _ => match hint {
            Placement::Home => push_home(shared, home, runnable),
            _ => shared.push_remote(runnable),
        },
    });
}

// ---------------------------------------------------------------------------
// yield_now
// ---------------------------------------------------------------------------

/// Reschedule the current task (local queue) and return control once.
pub fn yield_now() -> YieldNow {
    YieldNow { yielded: false }
}

pub struct YieldNow {
    yielded: bool,
}

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

pub struct Executor {
    shared: Arc<Shared>,
    handles: Vec<JoinHandle<WorkerExit>>,
}

/// What a worker thread hands back at exit.
struct WorkerExit {
    stats: WorkerStats,
    /// Bystander latency samples polled here, indexed by the worker whose
    /// queue the task had been scheduled onto; merged by `shutdown`.
    bystander_by_origin: Vec<Histogram>,
}

impl Executor {
    /// Start `workers` pinned worker threads with the default balancing
    /// interval. Panics if `workers` exceeds the number of physical cores or
    /// `stats::MAX_WORKERS`.
    pub fn new(workers: usize) -> Executor {
        Self::with_balance_interval(workers, DEFAULT_BALANCE_INTERVAL)
    }

    /// Like [`Executor::new`], with an explicit balancing interval: every
    /// `balance_interval` polls a worker moves half of a random peer's local
    /// queue into its own (`steal_batch`) even though it is not idle. `0`
    /// disables this, leaving only steal-when-idle plus the periodic injector
    /// check. Rationale: with one self-yielding bystander per worker no worker
    /// is ever idle, so without balancing every task woken with `Default`
    /// placement converges onto the waking worker (observed: all 64 clients on
    /// one worker under `dispatch`). The interval bounds how long a task can
    /// sit in a busy worker's local queue, so it also caps the measurable
    /// bystander delay (F2); it is recorded in the run config.
    pub fn with_balance_interval(workers: usize, balance_interval: u32) -> Executor {
        assert!(workers >= 1, "need at least one worker");
        assert!(
            workers <= MAX_WORKERS,
            "workers {workers} > MAX_WORKERS {MAX_WORKERS}"
        );
        let cores = physical_core_ids();
        assert!(
            workers <= cores.len(),
            "workers {workers} > physical cores {} (never run more workers than physical cores)",
            cores.len()
        );

        let locals: Vec<Worker<Runnable>> = (0..workers).map(|_| Worker::new_fifo()).collect();
        let stealers = locals.iter().map(|w| w.stealer()).collect();
        let (parkers, unparkers): (Vec<Parker>, Vec<Unparker>) =
            (0..workers).map(|_| parking::pair()).unzip();

        let shared = Arc::new(Shared {
            injector: Injector::new(),
            stealers,
            inboxes: (0..workers).map(|_| Injector::new()).collect(),
            unparkers,
            idle: Mutex::new(Vec::with_capacity(workers)),
            idle_count: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
            balance_interval,
        });
        for w in 0..workers {
            stats::take_combining(w);
        }

        let handles = locals
            .into_iter()
            .zip(parkers)
            .enumerate()
            .map(|(id, (local, parker))| {
                let shared = Arc::clone(&shared);
                let cpu = cores[id];
                std::thread::Builder::new()
                    .name(format!("coro-worker-{id}"))
                    .spawn(move || worker_main(id, cpu, local, parker, shared))
                    .expect("spawn worker thread")
            })
            .collect();

        Executor { shared, handles }
    }

    pub fn workers(&self) -> usize {
        self.handles.len()
    }

    /// Spawn `fut` as a task of `kind`. From a worker thread the first
    /// schedule goes to that worker's local queue, otherwise to the injector.
    pub fn spawn<F>(&self, kind: TaskKind, fut: F) -> Task<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let (runnable, task) = self.make_task(kind, fut);
        runnable.schedule();
        task
    }

    /// Spawn `fut` so that its first poll happens on `worker` (the runnable is
    /// delivered through that worker's inbox into its local queue). Later
    /// wakes follow the normal placement rules, so the task may migrate.
    pub fn spawn_on<F>(&self, worker: usize, kind: TaskKind, fut: F) -> Task<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        assert!(
            worker < self.workers(),
            "spawn_on: worker {worker} out of range"
        );
        let (runnable, task) = self.make_task(kind, fut);
        let meta = runnable.metadata();
        meta.scheduled_at.store(cycles(), Ordering::Relaxed);
        meta.scheduled_on.store(worker, Ordering::Relaxed);
        self.shared.inboxes[worker].push(runnable);
        self.shared.unpark_worker(worker);
        task
    }

    fn make_task<F>(&self, kind: TaskKind, fut: F) -> (Runnable, Task<F::Output>)
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        build_task(&self.shared, kind, fut)
    }

    /// Drive `fut` to completion on the calling thread (must not be a worker).
    pub fn block_on<F: Future>(&self, fut: F) -> F::Output {
        assert!(
            worker_id().is_none(),
            "block_on must not be called from a worker thread"
        );
        let (parker, unparker) = parking::pair();
        let waker = Waker::from(Arc::new(UnparkWaker(unparker)));
        let mut cx = Context::from_waker(&waker);
        let mut fut = std::pin::pin!(fut);
        loop {
            let r = fut.as_mut().poll(&mut cx);
            // A lock polled here may have set a hint meant for a worker.
            PLACEMENT_HINT.set(Placement::Default);
            match r {
                Poll::Ready(v) => return v,
                Poll::Pending => parker.park(),
            }
        }
    }

    /// Stop all workers (dropping any runnable still queued, i.e. cancelling
    /// unfinished tasks) and return their statistics ordered by worker id.
    /// Each worker's `bystander_latency` covers bystander tasks that were
    /// scheduled onto *that* worker's queues, whichever worker polled them.
    pub fn shutdown(self) -> Vec<WorkerStats> {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        for u in &self.shared.unparkers {
            u.unpark();
        }
        let mut exits: Vec<WorkerExit> = self
            .handles
            .into_iter()
            .map(|h| h.join().expect("worker thread panicked"))
            .collect();
        exits.sort_by_key(|e| e.stats.worker);
        let mut out: Vec<WorkerStats> = exits.iter().map(|e| e.stats.clone()).collect();
        for e in &exits {
            for (origin, h) in e.bystander_by_origin.iter().enumerate() {
                out[origin].bystander_latency.merge(h);
            }
        }
        out
    }
}

struct UnparkWaker(Unparker);

impl Wake for UnparkWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

// ---------------------------------------------------------------------------
// Worker loop
// ---------------------------------------------------------------------------

/// Per-worker mutable state that `poll_one` updates.
struct WorkerLocal {
    stats: WorkerStats,
    bystander_by_origin: Vec<Histogram>,
}

fn worker_main(
    id: usize,
    cpu: Option<usize>,
    local: Worker<Runnable>,
    parker: Parker,
    shared: Arc<Shared>,
) -> WorkerExit {
    let pinned = cpu.filter(|&c| core_affinity::set_for_current(core_affinity::CoreId { id: c }));
    let workers = shared.stealers.len();
    let mut wl = WorkerLocal {
        stats: WorkerStats::new(id, pinned),
        bystander_by_origin: (0..workers).map(|_| Histogram::new()).collect(),
    };

    CTX.with(|c| {
        let ok = c
            .set(WorkerCtx {
                id,
                local,
                run_next: Cell::new(None),
                current_inline: Cell::new(false),
                chain_len: Cell::new(0),
                chain_start: Cell::new(0),
                placements: [const { Cell::new(0) }; Placement::COUNT],
                shared: Arc::clone(&shared),
            })
            .is_ok();
        assert!(ok, "worker context already set");
    });

    let mut tick: u32 = 0;
    let mut rng =
        XorShift64(0x9E37_79B9_7F4A_7C15 ^ (id as u64 + 1).wrapping_mul(0x2545_F491_4F6C_DD1D));
    let balance = shared.balance_interval;

    CTX.with(|c| {
        let ctx = c.get().unwrap();
        let inbox = &shared.inboxes[id];
        loop {
            if shared.shutdown.load(Ordering::Relaxed) {
                break;
            }
            // 0. inbox (`spawn_on`): moved to the local queue on every
            //    iteration (two L1-hot loads) so a task spawned onto a worker
            //    that is inside a run-next chain becomes visible to stealers
            //    instead of sitting in an unstealable inbox.
            if !inbox.is_empty() {
                drain_inbox(ctx);
            }
            // 1. run-next slot: deterministic, before anything else.
            if let Some(r) = ctx.run_next.take() {
                poll_one(ctx, &mut wl, r);
                continue;
            }
            tick = tick.wrapping_add(1);
            // 2. periodic injector check so remote tasks cannot starve behind
            //    a self-yielding local task. Drain up to len/W + 1 items
            //    (tokio's share rule) so the drain rate scales with the
            //    backlog, but poll each one immediately instead of parking a
            //    batch in the local queue: a parked batch would be trapped
            //    if this worker then enters a run-next chain.
            if tick % INJECTOR_CHECK_INTERVAL == 0 {
                let limit = shared.injector.len() / workers + 1;
                let mut taken = 0;
                while taken < limit {
                    let Some(r) = steal_injector(ctx) else { break };
                    taken += 1;
                    wl.stats.steals += 1;
                    poll_one(ctx, &mut wl, r);
                    if ctx.run_next_pending() {
                        break;
                    }
                }
                if taken > 0 {
                    continue;
                }
            }
            // 3. periodic balancing steal (batch to the back of the local queue).
            if balance != 0 && tick % balance == 0 && balance_steal(ctx, &mut rng) {
                wl.stats.balance_steals += 1;
            }
            // 4. local FIFO queue.
            if let Some(r) = ctx.local.pop() {
                poll_one(ctx, &mut wl, r);
                continue;
            }
            // 5. idle: steal from the injector, then from other workers.
            if let Some(r) = find_work(ctx, &mut rng) {
                wl.stats.steals += 1;
                poll_one(ctx, &mut wl, r);
                continue;
            }
            // 6. park.
            if park(ctx, &parker) {
                wl.stats.parks += 1;
            }
        }
    });

    CTX.with(|c| {
        let ctx = c.get().unwrap();
        // A chain still open at exit counts as ended here.
        let len = ctx.chain_len.get();
        if len > 0 {
            wl.stats.chain_lengths.record(u64::from(len));
        }
        for (i, p) in ctx.placements.iter().enumerate() {
            wl.stats.placements[i] = p.get();
        }
    });
    wl.stats.combining_cycles = stats::take_combining(id);
    WorkerExit {
        stats: wl.stats,
        bystander_by_origin: wl.bystander_by_origin,
    }
}

#[inline]
fn poll_one(ctx: &WorkerCtx, wl: &mut WorkerLocal, runnable: Runnable) {
    let meta = runnable.metadata();
    let kind = meta.kind;
    let inline = meta.inline_resumed.swap(false, Ordering::Relaxed);
    ctx.current_inline.set(inline);
    meta.last_worker.store(ctx.id, Ordering::Relaxed);
    let recording = stats::recording();
    let start = cycles();
    // Chain bookkeeping: consecutive inline-resumed polls on this worker.
    if inline {
        let len = ctx.chain_len.get();
        if len == 0 {
            ctx.chain_start.set(start);
        }
        ctx.chain_len.set(len + 1);
    } else {
        let len = ctx.chain_len.get();
        if len > 0 {
            if recording {
                wl.stats.chain_lengths.record(u64::from(len));
            }
            ctx.chain_len.set(0);
        }
    }
    if recording && kind == TaskKind::Bystander {
        let t0 = meta.scheduled_at.load(Ordering::Relaxed);
        let origin = meta.scheduled_on.load(Ordering::Relaxed);
        let origin = if origin == NO_WORKER { ctx.id } else { origin };
        wl.bystander_by_origin[origin].record(start.saturating_sub(t0));
    }
    runnable.run();
    if recording {
        let end = cycles();
        let k = kind.index();
        wl.stats.poll_cycles_by_kind[k] += end.saturating_sub(start);
        wl.stats.polls_by_kind[k] += 1;
    }
}

/// Single-item injector steal. Batches are deliberately not used here: a
/// batch pulled into a worker's local queue right before that worker enters
/// a run-next chain is trapped there for the chain's duration (observed: up
/// to 25 of 64 clients starved for a whole window at balance_interval 0).
fn steal_injector(ctx: &WorkerCtx) -> Option<Runnable> {
    loop {
        match ctx.shared.injector.steal() {
            Steal::Success(r) => return Some(r),
            Steal::Empty => return None,
            Steal::Retry => {}
        }
    }
}

/// Move half of a random peer's local queue to the back of ours.
fn balance_steal(ctx: &WorkerCtx, rng: &mut XorShift64) -> bool {
    let shared = &*ctx.shared;
    let n = shared.stealers.len();
    if n < 2 {
        return false;
    }
    let mut victim = (rng.next() % (n as u64 - 1)) as usize;
    if victim >= ctx.id {
        victim += 1;
    }
    loop {
        match shared.stealers[victim].steal_batch(&ctx.local) {
            Steal::Success(()) => return true,
            Steal::Empty => return false,
            Steal::Retry => {}
        }
    }
}

/// Move everything in this worker's inbox to the back of its local queue.
fn drain_inbox(ctx: &WorkerCtx) {
    let inbox = &ctx.shared.inboxes[ctx.id];
    loop {
        match inbox.steal_batch(&ctx.local) {
            Steal::Success(()) | Steal::Retry => {
                if inbox.is_empty() {
                    return;
                }
            }
            Steal::Empty => return,
        }
    }
}

fn find_work(ctx: &WorkerCtx, rng: &mut XorShift64) -> Option<Runnable> {
    let shared = &*ctx.shared;
    let n = shared.stealers.len();
    drain_inbox(ctx);
    if let Some(r) = ctx.local.pop() {
        return Some(r);
    }
    loop {
        let mut retry = false;
        match shared.injector.steal() {
            Steal::Success(r) => return Some(r),
            Steal::Retry => retry = true,
            Steal::Empty => {}
        }
        let start = (rng.next() % n as u64) as usize;
        for i in 0..n {
            let j = (start + i) % n;
            if j == ctx.id {
                continue;
            }
            match shared.stealers[j].steal_batch_and_pop(&ctx.local) {
                Steal::Success(r) => return Some(r),
                Steal::Retry => retry = true,
                Steal::Empty => {}
            }
        }
        if !retry {
            return None;
        }
    }
}

/// Register as idle, re-check for work, and park. Returns true if it parked.
fn park(ctx: &WorkerCtx, parker: &Parker) -> bool {
    let shared = &*ctx.shared;
    shared.idle.lock().push(ctx.id);
    shared.idle_count.fetch_add(1, Ordering::SeqCst);
    fence(Ordering::SeqCst);
    if shared.has_work(ctx.id) || shared.shutdown.load(Ordering::SeqCst) {
        let mut idle = shared.idle.lock();
        if let Some(pos) = idle.iter().position(|&x| x == ctx.id) {
            idle.swap_remove(pos);
            shared.idle_count.fetch_sub(1, Ordering::SeqCst);
        }
        // else: a producer already popped us and will (or did) unpark; the
        // sticky token makes the next park return immediately.
        return false;
    }
    parker.park();
    true
}

struct XorShift64(u64);

impl XorShift64 {
    #[inline]
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

// ---------------------------------------------------------------------------
// Topology
// ---------------------------------------------------------------------------

/// One logical CPU per physical core (the lowest-numbered SMT sibling),
/// ascending. Falls back to every logical CPU if sysfs topology is missing.
/// On the study machine (2 x 32 cores, SMT siblings i and i+64) this yields
/// 0..64.
pub fn physical_core_ids() -> Vec<Option<usize>> {
    let Some(ids) = core_affinity::get_core_ids() else {
        return vec![None; MAX_WORKERS];
    };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut logical: Vec<usize> = ids.iter().map(|c| c.id).collect();
    logical.sort_unstable();
    for cpu in logical {
        let path = format!("/sys/devices/system/cpu/cpu{cpu}/topology/thread_siblings_list");
        let rep = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| parse_first_cpu(&s))
            .unwrap_or(cpu);
        if seen.insert(rep) {
            out.push(Some(rep));
        }
    }
    out
}

fn parse_first_cpu(list: &str) -> Option<usize> {
    list.trim()
        .split(',')
        .next()?
        .split('-')
        .next()?
        .trim()
        .parse()
        .ok()
}
