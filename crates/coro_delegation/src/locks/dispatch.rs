//! `dispatch`: FIFO mutex with tokio-style handoff.
//!
//! `unlock` pops the head waiter, marks its node `granted` (ownership is
//! transferred, the lock stays held) and calls its `Waker` with the default
//! placement: the scheduler decides where the new owner runs; the unlocking
//! task continues. This file also hosts the intrusive [`WaitQueue`] shared
//! with `ces`.
//!
//! ## Memory model
//!
//! - The queue state (`head`, `tail`, `locked`, node links, node wakers) is
//!   guarded by a TTAS spinlock (`compare_exchange(false, true, Acquire,
//!   Relaxed)` / `store(false, Release)`). Hold times are a few dozen cycles.
//! - Handoff edge: the unlocker's critical section precedes (program order)
//!   `granted.store(true, Release)`; the waiter's `granted.load(Acquire)`
//!   that observes `true` therefore orders the previous critical section
//!   before the waiter's own. No reliance on the executor's wake/poll
//!   synchronisation is needed.
//! - Uncontended acquire observes `locked == false` under the spinlock; the
//!   spinlock's Release/Acquire orders the previous unlock before it.
//! - `data` is touched only by the current owner (exclusive by protocol).
//!
//! ## Node lifetime
//!
//! Each client owns one `Node` (no per-request allocation). The queue holds
//! raw pointers to nodes of clients that are mutably borrowed by an
//! in-flight `run` future, so the node cannot move while linked. Dropping a
//! `run` future while queued unlinks the node (or, if ownership was already
//! granted, releases the lock), so the pointer never dangles unless the
//! future is leaked with `mem::forget` (excluded by the contract).
//! A closure that panics leaves the lock held (no poisoning).

use std::cell::UnsafeCell;
use std::future::Future;
use std::pin::Pin;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use crate::executor::{self, Placement};
use crate::lock::{cycles, DelegationLock, LockClient};

// ---------------------------------------------------------------------------
// Intrusive FIFO wait queue (shared by dispatch and ces)
// ---------------------------------------------------------------------------

/// Per-client intrusive waiter node.
pub(crate) struct Node {
    /// Guarded by the queue spinlock.
    next: UnsafeCell<*const Node>,
    /// Guarded by the queue spinlock.
    waker: UnsafeCell<Option<Waker>>,
    /// Release by the unlocker (under the spinlock), Acquire by the waiter.
    granted: AtomicBool,
}

// SAFETY: raw pointer and waker fields are only touched under the queue
// spinlock, or by the owning client before the node is published.
unsafe impl Send for Node {}

impl Node {
    pub(crate) fn new() -> Self {
        Node {
            next: UnsafeCell::new(ptr::null()),
            waker: UnsafeCell::new(None),
            granted: AtomicBool::new(false),
        }
    }
}

struct QueueState {
    head: *const Node,
    tail: *const Node,
    locked: bool,
}

/// FIFO mutex core: spinlock-guarded waiter list plus the protected data.
pub(crate) struct WaitQueue<T> {
    spin: AtomicBool,
    state: UnsafeCell<QueueState>,
    data: UnsafeCell<T>,
}

// SAFETY: `state` is only accessed under `spin`; `data` only by the lock
// owner. Raw node pointers refer to nodes kept alive by borrowed clients.
unsafe impl<T: Send> Send for WaitQueue<T> {}
unsafe impl<T: Send> Sync for WaitQueue<T> {}

impl<T> WaitQueue<T> {
    pub(crate) fn new(data: T) -> Self {
        WaitQueue {
            spin: AtomicBool::new(false),
            state: UnsafeCell::new(QueueState {
                head: ptr::null(),
                tail: ptr::null(),
                locked: false,
            }),
            data: UnsafeCell::new(data),
        }
    }

    #[inline]
    fn lock_spin(&self) {
        loop {
            if self
                .spin
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
            while self.spin.load(Ordering::Relaxed) {
                std::hint::spin_loop();
            }
        }
    }

    #[inline]
    fn unlock_spin(&self) {
        self.spin.store(false, Ordering::Release);
    }

    /// Pointer to the protected data; caller must be the current owner.
    #[inline]
    pub(crate) fn data(&self) -> *mut T {
        self.data.get()
    }

    /// Fast path or enqueue. Returns `true` if the lock was acquired; else
    /// `node` is now published at the tail with `cx`'s waker.
    pub(crate) fn acquire_or_enqueue(&self, node: &Node, cx: &Context<'_>) -> bool {
        node.granted.store(false, Ordering::Relaxed);
        // SAFETY: node is not yet visible to other threads.
        unsafe { *node.next.get() = ptr::null() };
        self.lock_spin();
        // SAFETY: spinlock held.
        let st = unsafe { &mut *self.state.get() };
        if !st.locked {
            st.locked = true;
            self.unlock_spin();
            return true;
        }
        // SAFETY: node is still private; the waker clone is an atomic
        // refcount increment (no allocation).
        unsafe { *node.waker.get() = Some(cx.waker().clone()) };
        if st.tail.is_null() {
            st.head = node;
        } else {
            // SAFETY: tail is a live node, spinlock held.
            unsafe { *(*st.tail).next.get() = node };
        }
        st.tail = node;
        self.unlock_spin();
        false
    }

    /// Subsequent poll of a queued waiter. Returns `true` once ownership has
    /// been granted; otherwise refreshes the stored waker.
    pub(crate) fn poll_granted(&self, node: &Node, cx: &Context<'_>) -> bool {
        if node.granted.load(Ordering::Acquire) {
            return true;
        }
        self.lock_spin();
        if node.granted.load(Ordering::Acquire) {
            self.unlock_spin();
            return true;
        }
        // SAFETY: spinlock held, node is linked and not granted.
        unsafe {
            let w = &mut *node.waker.get();
            if !w.as_ref().is_some_and(|w| w.will_wake(cx.waker())) {
                *w = Some(cx.waker().clone());
            }
        }
        self.unlock_spin();
        false
    }

    /// Unlock. With no waiter the lock becomes free and `None` is returned.
    /// Otherwise ownership passes to the head waiter (the lock stays held)
    /// and its waker is returned for the caller to wake after this call.
    pub(crate) fn release(&self) -> Option<Waker> {
        self.lock_spin();
        // SAFETY: spinlock held.
        let st = unsafe { &mut *self.state.get() };
        let head = st.head;
        if head.is_null() {
            st.locked = false;
            self.unlock_spin();
            return None;
        }
        // SAFETY: head is a live linked node, spinlock held.
        let waker = unsafe {
            st.head = *(*head).next.get();
            if st.head.is_null() {
                st.tail = ptr::null();
            }
            (*(*head).waker.get()).take()
        };
        // Release pairs with the waiter's Acquire load in `poll_granted`.
        // SAFETY: head is live (its client is borrowed by a pending future).
        unsafe { (*head).granted.store(true, Ordering::Release) };
        self.unlock_spin();
        waker
    }

    /// Cancellation of a queued request: unlink `node`, or if ownership had
    /// already been granted, release the lock. Returns a waker to wake.
    pub(crate) fn cancel(&self, node: &Node) -> Option<Waker> {
        self.lock_spin();
        if node.granted.load(Ordering::Acquire) {
            self.unlock_spin();
            return self.release();
        }
        // SAFETY: spinlock held; walk the list to unlink `node`.
        unsafe {
            let st = &mut *self.state.get();
            let target: *const Node = node;
            let mut prev: *const Node = ptr::null();
            let mut cur = st.head;
            while !cur.is_null() && cur != target {
                prev = cur;
                cur = *(*cur).next.get();
            }
            if cur == target {
                let next = *(*cur).next.get();
                if prev.is_null() {
                    st.head = next;
                } else {
                    *(*prev).next.get() = next;
                }
                if st.tail == target {
                    st.tail = prev;
                }
                *node.waker.get() = None;
            }
        }
        self.unlock_spin();
        None
    }
}

// ---------------------------------------------------------------------------
// dispatch
// ---------------------------------------------------------------------------

pub struct Dispatch<T> {
    queue: WaitQueue<T>,
    /// Placement of the handoff wake (`Default` = tokio-style local queue;
    /// `Inline` is not allowed here and is treated as `Default`).
    placement: Placement,
}

impl<T: Send + 'static> Dispatch<T> {
    pub fn with_placement(data: T, placement: Placement) -> Self {
        Dispatch {
            queue: WaitQueue::new(data),
            placement: match placement {
                Placement::Inline => Placement::Default,
                p => p,
            },
        }
    }

    pub fn placement(&self) -> Placement {
        self.placement
    }
}

pub struct DispatchClient<T> {
    lock: Arc<Dispatch<T>>,
    node: Node,
    usage: u64,
}

impl<T: Send + 'static> DelegationLock<T> for Dispatch<T> {
    type Client = DispatchClient<T>;

    fn new(data: T) -> Self {
        Self::with_placement(data, Placement::Default)
    }

    fn client(self: &Arc<Self>) -> DispatchClient<T> {
        DispatchClient {
            lock: Arc::clone(self),
            node: Node::new(),
            usage: 0,
        }
    }

    fn name() -> &'static str {
        "dispatch"
    }
}

impl<T: Send + 'static> LockClient<T> for DispatchClient<T> {
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
        self.usage
    }
}

#[derive(PartialEq, Eq)]
enum State {
    Init,
    Queued,
    Done,
}

struct Run<'a, T, F> {
    client: &'a mut DispatchClient<T>,
    f: Option<F>,
    state: State,
}

impl<T, F> Unpin for Run<'_, T, F> {}

impl<T, R, F> Run<'_, T, F>
where
    F: FnOnce(&mut T) -> R,
{
    /// Run the critical section as owner, then hand off.
    fn critical(&mut self) -> R {
        let f = self.f.take().expect("closure taken twice");
        let queue = &self.client.lock.queue;
        let t0 = cycles();
        // SAFETY: we own the lock.
        let r = f(unsafe { &mut *queue.data() });
        self.client.usage += cycles().wrapping_sub(t0);
        self.state = State::Done;
        if let Some(w) = queue.release() {
            executor::wake_with(self.client.lock.placement, &w);
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
        let queue = &this.client.lock.queue;
        match this.state {
            State::Init => {
                if queue.acquire_or_enqueue(&this.client.node, cx) {
                    Poll::Ready(this.critical())
                } else {
                    this.state = State::Queued;
                    Poll::Pending
                }
            }
            State::Queued => {
                if queue.poll_granted(&this.client.node, cx) {
                    Poll::Ready(this.critical())
                } else {
                    Poll::Pending
                }
            }
            State::Done => panic!("dispatch::Run polled after completion"),
        }
    }
}

impl<T, F> Drop for Run<'_, T, F> {
    fn drop(&mut self) {
        if self.state == State::Queued {
            if let Some(w) = self.client.lock.queue.cancel(&self.client.node) {
                w.wake();
            }
        }
    }
}
