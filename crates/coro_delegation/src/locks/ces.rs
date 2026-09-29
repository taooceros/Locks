//! `ces`: FIFO mutex with CES-style placement (König et al., arXiv:2511.09194).
//!
//! Same intrusive wait queue as `dispatch` ([`super::dispatch::WaitQueue`]).
//! The difference is what `unlock` does when a waiter exists:
//!
//! (b) the head waiter is resumed **inline** on the unlocking worker
//!     ([`executor::wake_inline`] -> run-next slot), and
//! (a) the unlocking task suspends and is rescheduled **remotely**
//!     ([`executor::reschedule_self_remote`] -> injector + unpark), returning
//!     `Pending` with its result parked in the future; the next poll returns
//!     it.
//!
//! The unlocking worker thus becomes the combiner thread for as long as the
//! queue stays non-empty: each inline-resumed waiter runs its critical section
//! on that worker and repeats the handoff. With no waiter the owner simply
//! continues (nothing to resume; no suspension).
//!
//! Placement is conveyed through the executor's thread-local placement hint
//! (see `executor.rs`): a `Waker` alone cannot express it.
//!
//! Combining attribution: critical-section cycles of a task that was
//! inline-resumed onto this worker are charged to the worker via
//! `stats::record_combining` (the executor exposes the per-task flag through
//! `executor::current_task_inline_resumed`). Queue administration is not
//! included. `LockClient::combining_cycles` is 0: clients never run other
//! clients' closures; the worker absorbs the burden instead.
//!
//! Burden under the uniform definition shared with `co_mutex`
//! ([`stats::record_foreign_cs`]): the closure's cycles are also charged to
//! the executing worker when the request was made on another worker. This
//! differs from the combining charge above when an inline-resumed request
//! had queued on the combiner's own worker (combining, not foreign) and when
//! a non-inline grant runs away from its home, e.g. a chain-break wake with
//! `default` placement or a grantee moved by a balancing steal (foreign, not
//! combining).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use super::dispatch::{Node, WaitQueue};
use crate::executor::{self, Placement};
use crate::lock::{cycles, DelegationLock, LockClient};
use crate::stats;

/// Chain-bounding mitigations (RESEARCH.md H-D). A *chain* is the sequence
/// of inline resumes on one worker since it last polled anything else
/// (`executor::current_chain`). When the chain reaches `chain_bound`
/// handoffs or `chain_budget_cycles` cycles, the unlocker hands ownership to
/// the head waiter with `break_placement` instead of `wake_inline` and
/// simply continues (no remote self-reschedule: the worker drains its own
/// queue once this poll returns). Inline resumes inside a chain stay inline.
/// `Default` = plain CES (no bound).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CesOptions {
    pub chain_bound: Option<u32>,
    pub chain_budget_cycles: Option<u64>,
    /// Placement of the chain-break handoff (`Inline` is treated as
    /// `Default`).
    pub break_placement: Placement,
}

pub struct Ces<T> {
    queue: WaitQueue<T>,
    opts: CesOptions,
}

impl<T: Send + 'static> Ces<T> {
    pub fn with_options(data: T, opts: CesOptions) -> Self {
        let break_placement = match opts.break_placement {
            Placement::Inline => Placement::Default,
            p => p,
        };
        Ces {
            queue: WaitQueue::new(data),
            opts: CesOptions {
                break_placement,
                ..opts
            },
        }
    }

    pub fn options(&self) -> CesOptions {
        self.opts
    }
}

pub struct CesClient<T> {
    lock: Arc<Ces<T>>,
    node: Node,
    usage: u64,
}

impl<T: Send + 'static> DelegationLock<T> for Ces<T> {
    type Client = CesClient<T>;

    fn new(data: T) -> Self {
        Self::with_options(data, CesOptions::default())
    }

    fn client(self: &Arc<Self>) -> CesClient<T> {
        CesClient {
            lock: Arc::clone(self),
            node: Node::new(),
            usage: 0,
        }
    }

    fn name() -> &'static str {
        "ces"
    }
}

impl<T: Send + 'static> LockClient<T> for CesClient<T> {
    fn run<R, F>(&mut self, f: F) -> impl Future<Output = R> + Send
    where
        R: Send,
        F: FnOnce(&mut T) -> R + Send,
    {
        Run {
            client: self,
            f: Some(f),
            result: None,
            state: State::Init,
            home: NO_HOME,
        }
    }

    fn usage(&self) -> u64 {
        self.usage
    }
}

#[derive(PartialEq, Eq)]
enum State {
    Init,
    Queued,
    /// Critical section done, task suspended for remote rescheduling.
    Suspended,
    Done,
}

struct Run<'a, T, R, F> {
    client: &'a mut CesClient<T>,
    f: Option<F>,
    result: Option<R>,
    state: State,
    /// Worker polling the task when it requested the lock (`NO_HOME` off
    /// the executor): the request's home for [`stats::record_foreign_cs`].
    home: usize,
}

const NO_HOME: usize = usize::MAX;

impl<T, R, F> Unpin for Run<'_, T, R, F> {}

impl<T, R, F> Run<'_, T, R, F>
where
    F: FnOnce(&mut T) -> R,
{
    /// True when this unlock happens inside a chain that has reached the
    /// configured handoff count or cycle budget.
    fn chain_bound_reached(&self) -> bool {
        let opts = &self.client.lock.opts;
        if opts.chain_bound.is_none() && opts.chain_budget_cycles.is_none() {
            return false;
        }
        let (len, start) = executor::current_chain();
        if len == 0 {
            return false; // not inline-resumed: not inside a chain
        }
        opts.chain_bound.is_some_and(|k| len >= k)
            || opts
                .chain_budget_cycles
                .is_some_and(|t| cycles().wrapping_sub(start) >= t)
    }

    /// Owner path: run the critical section, then CES handoff.
    fn critical(&mut self, cx: &Context<'_>) -> Poll<R> {
        let f = self.f.take().expect("closure taken twice");
        let queue = &self.client.lock.queue;
        let inline = executor::current_task_inline_resumed();
        let t0 = cycles();
        // SAFETY: we own the lock.
        let r = f(unsafe { &mut *queue.data() });
        let dt = cycles().wrapping_sub(t0);
        self.client.usage += dt;
        if let Some(w) = executor::worker_id() {
            if inline {
                stats::record_combining(w, dt);
            }
            if self.home != NO_HOME && self.home != w {
                stats::record_foreign_cs(w, dt);
            }
        }
        match queue.release() {
            None => {
                self.state = State::Done;
                Poll::Ready(r)
            }
            Some(waker) if self.chain_bound_reached() => {
                // Chain-break handoff: the head waiter is placed off the
                // inline slot and this task continues; the worker drains its
                // local queue after this poll.
                executor::wake_with(self.client.lock.opts.break_placement, &waker);
                self.state = State::Done;
                Poll::Ready(r)
            }
            Some(waker) => {
                // (b) next waiter runs next on this worker ...
                executor::wake_inline(&waker);
                // (a) ... and we go through the injector to another worker.
                executor::reschedule_self_remote(cx);
                self.result = Some(r);
                self.state = State::Suspended;
                Poll::Pending
            }
        }
    }
}

impl<T, R, F> Future for Run<'_, T, R, F>
where
    F: FnOnce(&mut T) -> R,
{
    type Output = R;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<R> {
        let this = self.get_mut();
        let queue = &this.client.lock.queue;
        match this.state {
            State::Init => {
                this.home = executor::worker_id().unwrap_or(NO_HOME);
                if queue.acquire_or_enqueue(&this.client.node, cx) {
                    this.critical(cx)
                } else {
                    this.state = State::Queued;
                    Poll::Pending
                }
            }
            State::Queued => {
                if queue.poll_granted(&this.client.node, cx) {
                    this.critical(cx)
                } else {
                    Poll::Pending
                }
            }
            State::Suspended => {
                this.state = State::Done;
                Poll::Ready(this.result.take().expect("result stored"))
            }
            State::Done => panic!("ces::Run polled after completion"),
        }
    }
}

impl<T, R, F> Drop for Run<'_, T, R, F> {
    fn drop(&mut self) {
        if self.state == State::Queued {
            if let Some(w) = self.client.lock.queue.cancel(&self.client.node) {
                w.wake();
            }
        }
    }
}
