//! ShflLock (AQS): Adaptive Queued Spinlock with NUMA-aware shuffling.
//!
//! A Rust port of the non-blocking AQS algorithm from ShflLock
//! (Kashyap et al., SOSP'19).  Implements [`lock_api::RawMutex`] so it
//! can be used with [`super::super::spinlock::DLock2Wrapper`].
//!
//! Algorithm overview:
//! - **Fast path**: test-and-set on `locked` byte (uncontended).
//! - **Slow path**: MCS-style queue with NUMA-aware shuffling of waiters.
//! - **Shuffling**: idle waiters reorder the queue off the critical path,
//!   grouping same-socket threads to reduce cache-line migration.

use std::{
    ptr::null_mut,
    sync::atomic::{AtomicPtr, AtomicU16, AtomicU8, Ordering},
};

use lock_api::{GuardSend, RawMutex};
use thread_local::ThreadLocal;

use super::node::{QNode, STATUS_LOCKED, STATUS_WAIT};

/// Threshold mask for `keep_lock_local()` — controls shuffle probability.
const THRESHOLD: u32 = 0xffff;

/// Shuffle quota PRNG modulus (must be power of 2).
const UNLOCK_COUNT_THRESHOLD: u32 = 1024;

// ---------------------------------------------------------------------------
// NUMA topology detection (cached)
// ---------------------------------------------------------------------------

/// Detect the current core's NUMA node using `rdtscp`.
///
/// Matches the C `current_numa_node()`: reads core ID from TSC_AUX,
/// divides by cores-per-socket.
#[inline]
fn current_numa_node() -> i32 {
    let core: u32;
    unsafe {
        let mut _aux: u32 = 0;
        core::arch::x86_64::__rdtscp(&mut _aux);
        core = _aux & 0xFFF;
    }
    let cpu_number = TOPOLOGY.cpu_count;
    let numa_nodes = TOPOLOGY.numa_nodes;
    (core / (cpu_number / numa_nodes)) as i32
}

struct Topology {
    cpu_count: u32,
    numa_nodes: u32,
}

static TOPOLOGY: std::sync::LazyLock<Topology> = std::sync::LazyLock::new(|| {
    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);

    // Count NUMA nodes from sysfs.
    let mut nodes = 0u32;
    for i in 0..256 {
        let path = format!("/sys/devices/system/node/node{}", i);
        if std::path::Path::new(&path).exists() {
            nodes += 1;
        } else if i > 0 {
            break;
        }
    }
    if nodes == 0 {
        nodes = 1;
    }

    Topology {
        cpu_count,
        numa_nodes: nodes,
    }
});

// ---------------------------------------------------------------------------
// Per-thread XOR-shift PRNG for shuffle quota
// ---------------------------------------------------------------------------

std::thread_local! {
    static XOR_RV: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// XOR-shift PRNG matching the C `xor_random()`.
#[inline]
fn xor_random() -> u32 {
    XOR_RV.with(|cell| {
        let mut v = cell.get();
        if v == 0 {
            // Seed from thread ID.
            v = std::thread::current().id().as_u64().get() as u32;
            if v == 0 {
                v = 1;
            }
        }
        v ^= v << 6;
        v ^= v >> 21;
        v ^= v << 7;
        cell.set(v);
        v & (UNLOCK_COUNT_THRESHOLD - 1)
    })
}

/// Returns nonzero most of the time (keep lock local); zero to stop shuffling.
#[inline]
fn keep_lock_local() -> bool {
    (xor_random() & THRESHOLD) != 0
}

// ---------------------------------------------------------------------------
// Helpers for atomic byte access (matching C's no_stealing control)
// ---------------------------------------------------------------------------

/// Atomically read `locked` and `no_stealing` as a single u16.
/// Layout (little-endian x86): byte 0 = locked, byte 1 = no_stealing.
#[inline]
fn load_locked_no_stealing(locked: &AtomicU8, no_stealing: &AtomicU8) -> u16 {
    // The two bytes are adjacent in memory.  We do a single 16-bit atomic
    // load by reinterpreting the pointer.
    let ptr = locked as *const AtomicU8 as *const AtomicU16;
    unsafe { (*ptr).load(Ordering::Acquire) }
}

/// CAS on the combined locked+no_stealing u16 word.
#[inline]
fn cas_locked_no_stealing(locked: &AtomicU8, old: u16, new: u16) -> Result<u16, u16> {
    let ptr = locked as *const AtomicU8 as *const AtomicU16;
    unsafe { (*ptr).compare_exchange(old, new, Ordering::AcqRel, Ordering::Acquire) }
}

// ---------------------------------------------------------------------------
// RawShflLock
// ---------------------------------------------------------------------------

/// Raw ShflLock (AQS) implementing [`lock_api::RawMutex`].
#[derive(Debug)]
pub struct RawShflLock {
    tail: AtomicPtr<QNode>,
    /// Lock byte: 0 = unlocked, 1 = locked.
    locked: AtomicU8,
    /// No-stealing flag: 1 = stealing disabled (queue head waiting).
    no_stealing: AtomicU8,
    /// Per-thread queue nodes.
    local_node: ThreadLocal<QNode>,
}

// SAFETY: The lock provides synchronisation for all shared state.
unsafe impl Send for RawShflLock {}
unsafe impl Sync for RawShflLock {}

const _AQS_NOSTEAL_VAL: u8 = 1;

impl RawShflLock {
    #[inline]
    fn enable_stealing(&self) {
        self.no_stealing.store(0, Ordering::Release);
    }

    #[inline]
    fn disable_stealing(&self) {
        self.no_stealing.store(_AQS_NOSTEAL_VAL, Ordering::Release);
    }

    /// Core NUMA-aware queue reordering.
    ///
    /// Walks from `node.last_visited` forward, moving same-socket nodes
    /// right after the last same-socket node.  Passes shuffle leadership
    /// to the last same-socket node found.
    ///
    /// SAFETY: Called only by a single shuffle leader at a time while
    /// the node is live in the queue.
    unsafe fn shuffle_waiters(&self, node: *mut QNode, is_next_waiter: bool) {
        let nid = (*node).nid;
        let mut curr_locked_count = (*node).wcount.load(Ordering::Relaxed);

        let mut prev = (*node).last_visited.load(Ordering::Acquire);
        if prev.is_null() {
            prev = node;
        }

        let mut sleader: *mut QNode = null_mut();
        let mut last: *mut QNode = node;
        let mut qend: *mut QNode = null_mut();
        let mut one_shuffle = false;

        if curr_locked_count == 0 {
            curr_locked_count += 1;
            (*node).wcount.store(curr_locked_count, Ordering::Relaxed);
        }

        // Clear our own shuffle leader flag.
        (*node).sleader.store(false, Ordering::Relaxed);

        // Probabilistic exit: occasionally stop shuffling to let
        // remote-socket threads through (starvation prevention).
        if !keep_lock_local() {
            sleader = (*node).next.load(Ordering::Acquire);
            if !sleader.is_null() {
                (*sleader).sleader.store(true, Ordering::Release);
            }
            return;
        }

        loop {
            let curr = (*prev).next.load(Ordering::Acquire);
            std::sync::atomic::fence(Ordering::Acquire);

            if curr.is_null() {
                sleader = last;
                qend = prev;
                break;
            }

            if curr == self.tail.load(Ordering::Acquire) {
                sleader = last;
                qend = prev;
                break;
            }

            if (*curr).nid == nid {
                if (*prev).nid == nid {
                    // Already adjacent — just mark batch.
                    (*curr).wcount.store(curr_locked_count, Ordering::Relaxed);
                    last = curr;
                    prev = curr;
                    one_shuffle = true;
                } else {
                    // Need to move curr after last same-socket node.
                    let next = (*curr).next.load(Ordering::Acquire);
                    if next.is_null() {
                        sleader = last;
                        qend = prev;
                        break;
                    }

                    (*curr).wcount.store(curr_locked_count, Ordering::Relaxed);
                    (*prev).next.store(next, Ordering::Release);
                    (*curr)
                        .next
                        .store((*last).next.load(Ordering::Relaxed), Ordering::Release);
                    (*last).next.store(curr, Ordering::Release);
                    last = curr;
                    one_shuffle = true;
                }
            } else {
                prev = curr;
            }

            // Early exit: lock became available or we were promoted.
            let lock_ready = self.locked.load(Ordering::Acquire) == 0;
            if one_shuffle
                && ((is_next_waiter && lock_ready)
                    || (!is_next_waiter
                        && (*node).lstatus.load(Ordering::Acquire) == STATUS_LOCKED))
            {
                sleader = last;
                qend = prev;
                break;
            }
        }

        // Pass shuffle leadership.
        if !sleader.is_null() {
            (*sleader).sleader.store(true, Ordering::Release);
            if qend != sleader && !qend.is_null() {
                (*sleader).last_visited.store(qend, Ordering::Release);
            }
        }
    }
}

unsafe impl RawMutex for RawShflLock {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = RawShflLock {
        tail: AtomicPtr::new(null_mut()),
        locked: AtomicU8::new(0),
        no_stealing: AtomicU8::new(0),
        local_node: ThreadLocal::new(),
    };

    type GuardMarker = GuardSend;

    fn lock(&self) {
        // Force topology init.
        let _ = &*TOPOLOGY;

        let node = self.local_node.get_or(QNode::new);
        let node_ptr: *mut QNode = node as *const QNode as *mut QNode;

        // --- Fast path: uncontended CAS on locked+no_stealing u16 ---
        if cas_locked_no_stealing(&self.locked, 0, 1).is_ok() {
            // Got the lock with no queue.  Hand off to successor if one
            // appeared while we were doing the CAS.
            unsafe {
                self.fast_path_successor_handoff(node_ptr);
            }
            return;
        }

        // --- Slow path: MCS-style enqueue ---
        unsafe {
            (*node_ptr).next.store(null_mut(), Ordering::Relaxed);
            // Clear all status bits (lstatus=0, sleader=0, wcount=0).
            (*node_ptr).lstatus.store(STATUS_WAIT, Ordering::Relaxed);
            (*node_ptr).sleader.store(false, Ordering::Relaxed);
            (*node_ptr).wcount.store(0, Ordering::Relaxed);
            (*node_ptr).nid = current_numa_node();
            (*node_ptr)
                .last_visited
                .store(null_mut(), Ordering::Relaxed);
        }

        let prev = self.tail.swap(node_ptr, Ordering::AcqRel);

        if !prev.is_null() {
            // Link into predecessor.
            unsafe {
                (*prev).next.store(node_ptr, Ordering::Release);
            }

            // Wait for lock holder to promote us to next-waiter.
            // Shuffle while waiting if elected.
            loop {
                if node.lstatus.load(Ordering::Acquire) == STATUS_LOCKED {
                    break;
                }
                if node.sleader.load(Ordering::Acquire) {
                    unsafe {
                        self.shuffle_waiters(node_ptr, false);
                    }
                }
                core::hint::spin_loop();
            }
        } else {
            // We are the only one in the queue — disable fast-path stealing.
            self.disable_stealing();
        }

        // --- Head-of-queue: spin for the lock byte, shuffle while waiting ---
        loop {
            if self.locked.load(Ordering::Acquire) == 0 {
                break;
            }
            let wcount = node.wcount.load(Ordering::Relaxed);
            if wcount == 0 || node.sleader.load(Ordering::Relaxed) {
                unsafe {
                    self.shuffle_waiters(node_ptr, true);
                }
            }
        }

        // CAS to acquire (fast-path stealers may race).
        loop {
            if self
                .locked
                .compare_exchange_weak(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
            while self.locked.load(Ordering::Acquire) != 0 {
                core::hint::spin_loop();
            }
        }

        // Hand off to successor.
        unsafe {
            if (*node_ptr).next.load(Ordering::Acquire).is_null() {
                if self
                    .tail
                    .compare_exchange(node_ptr, null_mut(), Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    self.enable_stealing();
                    return;
                }
                // Successor is linking — wait for it.
                while (*node_ptr).next.load(Ordering::Acquire).is_null() {
                    core::hint::spin_loop();
                }
            }
            let next = (*node_ptr).next.load(Ordering::Acquire);
            (*next).lstatus.store(STATUS_LOCKED, Ordering::Release);
        }
    }

    fn try_lock(&self) -> bool {
        self.locked
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    unsafe fn unlock(&self) {
        self.locked.store(0, Ordering::Release);
    }
}

impl RawShflLock {
    /// After the fast-path CAS succeeds, check if a successor enqueued
    /// concurrently and hand off to them.
    unsafe fn fast_path_successor_handoff(&self, node_ptr: *mut QNode) {
        // In the fast path we didn't enqueue, so there's nothing to hand off.
        // The successor (if any) will see locked==1 and wait; they'll be
        // woken when we unlock.  This matches the C code's `goto release`
        // after the fast-path CAS.
        let _ = node_ptr;
    }
}
