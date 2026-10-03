//! Shared contract for every lock variant in this crate.
//!
//! Every variant, mutex-style or delegation-style, is exercised through
//! `LockClient::run`: the critical section is a closure executed with
//! exclusive access to `T`. Mutex-style locks implement `run` as
//! `lock().await; f(&mut data)`; delegation locks ship the closure to a
//! combiner. The closure never awaits.
//!
//! Invariants every implementation MUST hold:
//! - No per-request heap allocation in steady state (a per-client node is fine).
//! - `run` futures are `Send` so tasks may migrate between executor workers.
//! - Dropping an unfinished `run` future is allowed only before it has been
//!   published to the lock (cancel-safety after publication is out of scope;
//!   the harness never cancels).
//! - `usage()` is the cumulative critical-section cost charged to this client
//!   in TSC cycles, or 0 for locks that do not account.

use std::future::Future;
use std::sync::Arc;

pub trait DelegationLock<T: Send + 'static>: Send + Sync + 'static {
    type Client: LockClient<T>;

    fn new(data: T) -> Self;

    /// One client per task (or per tenant, if the harness shares clients).
    fn client(self: &Arc<Self>) -> Self::Client;

    /// Human-readable variant name used in result files.
    fn name() -> &'static str;
}

pub trait LockClient<T: Send + 'static>: Send + 'static {
    fn run<R, F>(&mut self, f: F) -> impl Future<Output = R> + Send
    where
        R: Send,
        F: FnOnce(&mut T) -> R + Send;

    fn usage(&self) -> u64;

    /// Cumulative cycles this client's task spent acting as combiner
    /// (running other clients' closures plus queue administration).
    /// 0 for non-delegation locks.
    fn combining_cycles(&self) -> u64 {
        0
    }
}

/// `rdtscp`-based cycle counter shared by locks and harness.
#[inline]
pub fn cycles() -> u64 {
    let mut aux = 0u32;
    // SAFETY: rdtscp is available on every x86-64 target this crate supports.
    unsafe { core::arch::x86_64::__rdtscp(&mut aux) }
}

// ---------------------------------------------------------------------------
// Coroutine-style mutex (RESEARCH.md, "API assumption (2026-09-30)")
// ---------------------------------------------------------------------------

/// A task's view of a coroutine-style mutex. The critical section is the
/// task's own continuation, not a shipped closure:
///
/// ```text
/// let mut g = h.lock().await;   // h: this task's handle
/// g.insert(k, v);               // CS: any code on *g, may .await
/// g.unlock().await;             // explicit async release
/// ```
///
/// `unlock().await` is where an implementation may suspend the releaser
/// once (step aside) so that the next owner runs first. Dropping the guard
/// without `unlock` is a correct synchronous release. Holding the guard
/// across unrelated `.await`s is allowed: the owner stays owner and waiters
/// wait. Not reentrant: a second `lock()` by the current owner deadlocks.
///
/// Implemented by a per-task handle ([`CoLock::handle`]) rather than by
/// the mutex itself because usage-ordered policies charge each critical
/// section to a client, and the executor has no task-local storage. `lock`
/// takes `&self`: waiter nodes live in the `lock()` future (pinned while
/// queued), so concurrent `lock()` calls on one handle are sound; they
/// share its usage account.
pub trait AsyncMutex<T: Send + 'static>: Send + Sync + 'static {
    type Guard<'a>: AsyncGuard<T> + Send + 'a
    where
        Self: 'a;

    fn lock(&self) -> impl Future<Output = Self::Guard<'_>> + Send;

    /// Cumulative charged usage in TSC cycles (ownership observed by
    /// `lock()` to `unlock()` / drop), 0 for locks that do not account.
    fn usage(&self) -> u64;
}

/// Ownership of the protected value, released by [`AsyncGuard::unlock`] or
/// by `Drop`.
pub trait AsyncGuard<T>: std::ops::DerefMut<Target = T> {
    fn unlock(self) -> impl Future<Output = ()> + Send;
}

/// Constructor side of a coroutine-style mutex (the analogue of
/// [`DelegationLock`]).
pub trait CoLock<T: Send + 'static>: Send + Sync + 'static {
    type Handle: AsyncMutex<T>;

    fn new(data: T) -> Self;

    /// One handle per task: its identity for usage accounting.
    fn handle(self: &Arc<Self>) -> Self::Handle;

    /// Human-readable variant name used in result files.
    fn name() -> &'static str;
}
