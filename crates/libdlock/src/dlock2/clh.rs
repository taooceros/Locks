//! CLH (Craig, Landin, Hagersten) queue spin lock.
//!
//! Each thread maintains a local node and spins on its *predecessor's* node
//! (unlike MCS where threads spin on their own node).  On unlock, the thread
//! writes to its own node to signal the successor, then steals the
//! predecessor's node for the next acquisition.
//!
//! CLH is FIFO-fair and has O(1) remote memory references per handoff (one
//! write to signal the successor).  Compared to MCS:
//! - **Advantage**: simpler unlock (no next-pointer chase)
//! - **Disadvantage**: requires implicit node recycling; slightly worse NUMA
//!   behavior (spins on predecessor's cache line, which may be remote)
//!
//! Useful as an additional acquisition-fair baseline alongside MCS and Ticket.

use std::{
    cell::UnsafeCell,
    ptr::null_mut,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

use lock_api::{GuardSend, RawMutex};
use thread_local::ThreadLocal;

/// Per-thread queue node, cache-line aligned.
#[derive(Debug)]
#[repr(align(128))]
struct ClhNode {
    /// `true` while the owning thread holds or is waiting for the lock.
    locked: AtomicBool,
}

impl ClhNode {
    fn new_locked() -> *mut Self {
        Box::into_raw(Box::new(ClhNode {
            locked: AtomicBool::new(true),
        }))
    }

    fn new_unlocked() -> *mut Self {
        Box::into_raw(Box::new(ClhNode {
            locked: AtomicBool::new(false),
        }))
    }
}

unsafe impl Send for ClhNode {}
unsafe impl Sync for ClhNode {}

/// Thread-local state tracking the two nodes involved in the CLH protocol:
/// - `my_node`: the node this thread will swap into the tail on lock()
/// - `my_pred`: the predecessor's node (set during lock(), used in unlock()
///   for recycling)
#[derive(Debug)]
struct ClhThreadState {
    /// Pointer to this thread's current queue node.
    my_node: UnsafeCell<*mut ClhNode>,
    /// Pointer to the predecessor's node (set after acquiring the lock).
    my_pred: UnsafeCell<*mut ClhNode>,
}

impl ClhThreadState {
    fn new() -> Self {
        ClhThreadState {
            my_node: UnsafeCell::new(ClhNode::new_locked()),
            my_pred: UnsafeCell::new(null_mut()),
        }
    }
}

unsafe impl Send for ClhThreadState {}
unsafe impl Sync for ClhThreadState {}

/// Raw CLH queue spin lock, compatible with [`lock_api::RawMutex`].
///
/// Suitable for use with [`super::spinlock::DLock2Wrapper`].
#[derive(Debug)]
pub struct RawClhLock {
    /// Tail of the implicit queue. Points to the most recently enqueued
    /// thread's node, or a sentinel if the lock is free.
    tail: AtomicPtr<ClhNode>,
    local_state: ThreadLocal<ClhThreadState>,
}

unsafe impl Send for RawClhLock {}
unsafe impl Sync for RawClhLock {}

unsafe impl RawMutex for RawClhLock {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = RawClhLock {
        // Start with null; first lock() call installs a sentinel.
        tail: AtomicPtr::new(null_mut()),
        local_state: ThreadLocal::new(),
    };

    type GuardMarker = GuardSend;

    fn lock(&self) {
        let state = self.local_state.get_or(ClhThreadState::new);

        // SAFETY: only this thread accesses its own state.
        let my_node = unsafe { *state.my_node.get() };

        // Announce intent to acquire the lock.
        unsafe { (*my_node).locked.store(true, Ordering::Relaxed) };

        // Atomically enqueue: set tail to our node, get predecessor.
        let pred = self.tail.swap(my_node, Ordering::AcqRel);

        // If tail was null (very first lock() call), create a sentinel
        // predecessor with locked=false so we immediately acquire.
        let pred = if pred.is_null() {
            ClhNode::new_unlocked()
        } else {
            pred
        };

        // Spin on predecessor's locked flag.
        while unsafe { (*pred).locked.load(Ordering::Acquire) } {
            core::hint::spin_loop();
        }

        // Lock acquired. Save predecessor pointer for unlock().
        unsafe { *state.my_pred.get() = pred };
    }

    fn try_lock(&self) -> bool {
        let state = self.local_state.get_or(ClhThreadState::new);
        let my_node = unsafe { *state.my_node.get() };

        unsafe { (*my_node).locked.store(true, Ordering::Relaxed) };

        // Try to become the tail, but only if the queue is empty (tail is
        // null or points to an unlocked sentinel).
        let prev = self.tail.swap(my_node, Ordering::AcqRel);

        if prev.is_null() {
            // First call; install sentinel as predecessor.
            let sentinel = ClhNode::new_unlocked();
            unsafe { *state.my_pred.get() = sentinel };
            return true;
        }

        // Check if predecessor is unlocked (lock is free).
        if !unsafe { (*prev).locked.load(Ordering::Acquire) } {
            unsafe { *state.my_pred.get() = prev };
            return true;
        }

        // Lock is held — undo our enqueue by restoring tail.  This is only
        // safe if no successor has enqueued after us.
        let restored = self.tail.compare_exchange(
            my_node,
            prev,
            Ordering::AcqRel,
            Ordering::Relaxed,
        );
        if restored.is_ok() {
            // Successfully restored; we never entered the queue.
            unsafe { (*my_node).locked.store(false, Ordering::Relaxed) };
            false
        } else {
            // A successor has already enqueued after us — we're committed.
            // Must complete the lock acquisition (spin until predecessor
            // releases).
            while unsafe { (*prev).locked.load(Ordering::Acquire) } {
                core::hint::spin_loop();
            }
            unsafe { *state.my_pred.get() = prev };
            true
        }
    }

    unsafe fn unlock(&self) {
        let state = self.local_state.get_or(ClhThreadState::new);

        let my_node = *state.my_node.get();
        let my_pred = *state.my_pred.get();

        // Signal successor by clearing our node's locked flag.
        // The successor is spinning on `(*my_node).locked`.
        (*my_node).locked.store(false, Ordering::Release);

        // Recycle the predecessor's node for our next lock() call.
        // The predecessor no longer references it (it has already moved on
        // or exited).  Reset it to locked for the next acquisition.
        (*my_pred).locked.store(true, Ordering::Relaxed);
        *state.my_node.get() = my_pred;
    }
}
