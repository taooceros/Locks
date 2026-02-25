//! MCS (Mellor-Crummey & Scott) queue spin lock.
//!
//! A fair, FIFO queue lock. Each thread maintains a thread-local [`QNode`].
//! Acquiring the lock appends the thread's node to a shared tail queue and
//! spins locally on its own `locked` flag; releasing hands ownership to the
//! immediate successor.
//!
//! # Safety
//!
//! Raw pointer access to neighbouring queue nodes is safe because:
//! * A node's owner never exits the lock protocol until the node is dequeued.
//! * All cross-thread accesses use appropriately-ordered atomic operations.

use std::{
    ptr::null_mut,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

use lock_api::{GuardSend, RawMutex};

/// Per-thread queue node.
///
/// Padded to a full cache line to eliminate false sharing between adjacent
/// nodes in the queue.
#[repr(align(128))]
struct QNode {
    /// `true` while this thread is waiting to enter the critical section.
    locked: AtomicBool,
    /// Pointer to the next node installed after ours in the queue.
    next: AtomicPtr<QNode>,
}

impl QNode {
    const fn new() -> Self {
        QNode {
            locked: AtomicBool::new(false),
            next: AtomicPtr::new(null_mut()),
        }
    }
}

// SAFETY: All fields are atomic; cross-thread access is properly ordered.
unsafe impl Send for QNode {}
unsafe impl Sync for QNode {}

thread_local! {
    static QNODE: QNode = const { QNode::new() };
}

/// Raw MCS queue spin lock, compatible with [`lock_api::RawMutex`].
///
/// Suitable for use with [`super::spinlock::DLock2Wrapper`].
///
/// # Example
///
/// ```rust
/// use libdlock::dlock2::mcs::RawMcsLock;
/// use libdlock::dlock2::spinlock::DLock2Wrapper;
/// // DLock2Wrapper<u64, u64, _, RawMcsLock> can be constructed via
/// // DLock2Wrapper::new(data, delegate)
/// ```
#[derive(Debug)]
pub struct RawMcsLock {
    tail: AtomicPtr<QNode>,
}

// SAFETY: The tail pointer is manipulated solely via atomic operations and is
// safe to share across threads.
unsafe impl Send for RawMcsLock {}
unsafe impl Sync for RawMcsLock {}

unsafe impl RawMutex for RawMcsLock {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = RawMcsLock {
        tail: AtomicPtr::new(null_mut()),
    };

    type GuardMarker = GuardSend;

    fn lock(&self) {
        QNODE.with(|node| {
            let node_ptr: *mut QNode = node as *const QNode as *mut QNode;

            // Prepare this node: we intend to wait.
            node.locked.store(true, Ordering::Relaxed);
            node.next.store(null_mut(), Ordering::Relaxed);

            // Atomically enqueue ourselves at the tail.
            let prev = self.tail.swap(node_ptr, Ordering::AcqRel);
            if prev.is_null() {
                // Queue was empty — we immediately own the lock.
                return;
            }

            // Publish our node to the predecessor so it can hand off to us
            // during unlock.
            //
            // SAFETY: `prev` points to a live `QNode` owned by a thread that
            // is either spinning in `lock()` or about to inspect `next` in
            // `unlock()`.  The Release ordering ensures the `tail.swap` above
            // is visible before we touch `(*prev).next`.
            unsafe {
                (*prev).next.store(node_ptr, Ordering::Release);
            }

            // Spin locally until our predecessor clears our `locked` flag.
            while node.locked.load(Ordering::Acquire) {
                core::hint::spin_loop();
            }
        });
    }

    fn try_lock(&self) -> bool {
        QNODE.with(|node| {
            let node_ptr: *mut QNode = node as *const QNode as *mut QNode;

            // Initialise the node (safe to do before the CAS: if it fails,
            // no other thread will have seen the pointer).
            node.locked.store(false, Ordering::Relaxed);
            node.next.store(null_mut(), Ordering::Relaxed);

            // Succeed only when the queue is empty (no contention).
            self.tail
                .compare_exchange(null_mut(), node_ptr, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        })
    }

    unsafe fn unlock(&self) {
        QNODE.with(|node| {
            let node_ptr: *mut QNode = node as *const QNode as *mut QNode;

            // Fast path: we are the sole waiter — restore the empty state.
            if self
                .tail
                .compare_exchange(node_ptr, null_mut(), Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }

            // A successor is in the process of setting our `next` field.
            // Spin until it has done so.
            let next = loop {
                let p = node.next.load(Ordering::Acquire);
                if !p.is_null() {
                    break p;
                }
                core::hint::spin_loop();
            };

            // Grant ownership to the successor by clearing its `locked` flag.
            //
            // SAFETY: `next` points to a live `QNode` owned by our successor
            // thread, which is spinning on its `locked` flag in `lock()`.
            unsafe {
                (*next).locked.store(false, Ordering::Release);
            }
        });
    }
}
