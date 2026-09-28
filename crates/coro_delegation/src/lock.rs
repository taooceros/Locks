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
