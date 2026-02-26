//! CFL-MCS: Compact Fair Lock built on MCS queue.
//!
//! A usage-fair traditional lock that tracks per-thread cumulative lock-hold
//! time (vLHT) and reorders the MCS wait queue so the thread with the lowest
//! vLHT is served next. This is the key baseline for comparing against
//! delegation-based fair locks (FC-PQ, FC-Ban).
//!
//! The reordering is done inline by waiters while spinning (similar to
//! ShflLock's shuffle mechanism), rather than by a dedicated background thread.
//! This captures the essential cost: O(N) cross-core cache reads per queue
//! traversal to compare vLHT values.
//!
//! Reference: Park & Eom, "Locks as a Resource: Fairly Scheduling Lock
//! Occupation with CFL," PPoPP 2024.

use std::{
    ptr::null_mut,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering},
};

use lock_api::{GuardSend, RawMutex};
use thread_local::ThreadLocal;

// ---------------------------------------------------------------------------
// Per-thread queue node
// ---------------------------------------------------------------------------

/// Queue node for CFL-MCS. Extends the basic MCS node with a vLHT snapshot
/// for usage-fair reordering.
#[derive(Debug)]
#[repr(align(128))]
struct QNode {
    /// `true` while this thread is waiting to enter the critical section.
    locked: AtomicBool,
    /// Pointer to the next node in the queue.
    next: AtomicPtr<QNode>,
    /// Snapshot of the owning thread's cumulative lock-hold time (TSC cycles).
    /// The waiter at the head uses this to reorder the queue.
    vlht: AtomicU64,
}

impl QNode {
    fn new() -> Self {
        QNode {
            locked: AtomicBool::new(false),
            next: AtomicPtr::new(null_mut()),
            vlht: AtomicU64::new(0),
        }
    }
}

unsafe impl Send for QNode {}
unsafe impl Sync for QNode {}

// ---------------------------------------------------------------------------
// Per-thread vLHT state (thread-local, per-lock)
// ---------------------------------------------------------------------------

/// Per-thread usage accounting stored in the lock's ThreadLocal.
#[derive(Debug)]
#[repr(align(128))]
struct ThreadState {
    /// Cumulative lock-hold time in TSC cycles.
    vlht: AtomicU64,
    /// TSC timestamp at critical section start.
    cs_start: AtomicU64,
}

impl ThreadState {
    fn new() -> Self {
        ThreadState {
            vlht: AtomicU64::new(0),
            cs_start: AtomicU64::new(0),
        }
    }
}

unsafe impl Send for ThreadState {}
unsafe impl Sync for ThreadState {}

// ---------------------------------------------------------------------------
// RawCflLock
// ---------------------------------------------------------------------------

/// Raw CFL-MCS lock implementing [`lock_api::RawMutex`].
///
/// Each lock instance owns its own per-thread `QNode` and `ThreadState`,
/// so a thread can safely hold multiple CFL locks simultaneously.
#[derive(Debug)]
pub struct RawCflLock {
    tail: AtomicPtr<QNode>,
    local_node: ThreadLocal<QNode>,
    local_state: ThreadLocal<ThreadState>,
}

unsafe impl Send for RawCflLock {}
unsafe impl Sync for RawCflLock {}

impl RawCflLock {
    /// Reorder the wait queue by vLHT (lowest first).
    ///
    /// Performs a single bubble-sort pass from the head's successor forward.
    /// Moves the minimum-vLHT node to the front of the queue (right after the
    /// head). Each node read is a potential cross-core cache miss.
    ///
    /// SAFETY: Must be called only by the thread at the head of the queue,
    /// which is spinning and waiting for the lock. Other waiters are also
    /// spinning on their own `locked` flag.
    unsafe fn reorder_queue(&self, head: *mut QNode) {
        // Find the node with minimum vLHT in the queue after head.
        let first = (*head).next.load(Ordering::Acquire);
        if first.is_null() {
            return;
        }

        let mut min_vlht = (*first).vlht.load(Ordering::Relaxed);
        let mut min_node = first;
        let mut min_prev: *mut QNode = head;

        let mut prev = first;
        loop {
            let curr = (*prev).next.load(Ordering::Acquire);
            if curr.is_null() {
                break;
            }
            // Don't traverse past the tail — the tail node's next may be
            // in flux as new threads enqueue.
            if curr == self.tail.load(Ordering::Acquire) {
                break;
            }

            let curr_vlht = (*curr).vlht.load(Ordering::Relaxed);
            if curr_vlht < min_vlht {
                min_vlht = curr_vlht;
                min_node = curr;
                min_prev = prev;
            }
            prev = curr;
        }

        // If the minimum is already at the front, nothing to do.
        if min_node == first {
            return;
        }

        // Pointer surgery: remove min_node from its current position and
        // insert it right after head (before first).
        let min_next = (*min_node).next.load(Ordering::Acquire);
        if min_next.is_null() {
            // min_node might be the tail and a new thread is linking in.
            // Don't move it to avoid races.
            return;
        }
        (*min_prev).next.store(min_next, Ordering::Relaxed);
        (*min_node).next.store(first, Ordering::Relaxed);
        (*head).next.store(min_node, Ordering::Relaxed);
    }
}

unsafe impl RawMutex for RawCflLock {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = RawCflLock {
        tail: AtomicPtr::new(null_mut()),
        local_node: ThreadLocal::new(),
        local_state: ThreadLocal::new(),
    };

    type GuardMarker = GuardSend;

    fn lock(&self) {
        let node = self.local_node.get_or(QNode::new);
        let state = self.local_state.get_or(ThreadState::new);
        let node_ptr: *mut QNode = node as *const QNode as *mut QNode;

        // Prepare node.
        node.locked.store(true, Ordering::Relaxed);
        node.next.store(null_mut(), Ordering::Relaxed);
        // Publish our vLHT so the reordering pass can see it.
        node.vlht
            .store(state.vlht.load(Ordering::Relaxed), Ordering::Relaxed);

        // Enqueue.
        let prev = self.tail.swap(node_ptr, Ordering::AcqRel);
        if prev.is_null() {
            // Queue was empty — we own the lock immediately.
            state
                .cs_start
                .store(rdtsc_start(), Ordering::Relaxed);
            return;
        }

        // Link ourselves to predecessor.
        unsafe {
            (*prev).next.store(node_ptr, Ordering::Release);
        }

        // Spin until predecessor hands off.
        // The predecessor (head of queue) reorders before handing off,
        // so by the time we are woken, we should be the min-vLHT thread.
        while node.locked.load(Ordering::Acquire) {
            core::hint::spin_loop();
        }

        // Record CS start time.
        state
            .cs_start
            .store(rdtsc_start(), Ordering::Relaxed);
    }

    fn try_lock(&self) -> bool {
        let node = self.local_node.get_or(QNode::new);
        let state = self.local_state.get_or(ThreadState::new);
        let node_ptr: *mut QNode = node as *const QNode as *mut QNode;

        node.locked.store(false, Ordering::Relaxed);
        node.next.store(null_mut(), Ordering::Relaxed);
        node.vlht
            .store(state.vlht.load(Ordering::Relaxed), Ordering::Relaxed);

        if self
            .tail
            .compare_exchange(null_mut(), node_ptr, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            state
                .cs_start
                .store(rdtsc_start(), Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    unsafe fn unlock(&self) {
        let node = self.local_node.get_or(QNode::new);
        let state = self.local_state.get_or(ThreadState::new);
        let node_ptr: *mut QNode = node as *const QNode as *mut QNode;

        // Accumulate lock-hold time.
        let cs_start = state.cs_start.load(Ordering::Relaxed);
        let cs_end = rdtsc_end();
        let cs_duration = cs_end.saturating_sub(cs_start);
        state.vlht.fetch_add(cs_duration, Ordering::Relaxed);

        // Fast path: sole waiter.
        if self
            .tail
            .compare_exchange(node_ptr, null_mut(), Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            return;
        }

        // Wait for successor to link in.
        loop {
            let p = node.next.load(Ordering::Acquire);
            if !p.is_null() {
                break;
            }
            core::hint::spin_loop();
        }

        // Reorder the queue from our node forward.
        // This is the CFL fairness mechanism: the unlocking thread
        // (which just finished its CS and is about to hand off) traverses
        // the queue and moves the min-vLHT node to the front.
        self.reorder_queue(node_ptr);

        // Hand off to the (possibly reordered) successor.
        let next = node.next.load(Ordering::Acquire);
        (*next).locked.store(false, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// TSC helpers
// ---------------------------------------------------------------------------

#[inline]
fn rdtsc_start() -> u64 {
    unsafe {
        let mut aux: u32 = 0;
        core::arch::x86_64::__rdtscp(&mut aux)
    }
}

#[inline]
fn rdtsc_end() -> u64 {
    unsafe {
        let mut aux: u32 = 0;
        core::arch::x86_64::__rdtscp(&mut aux)
    }
}
