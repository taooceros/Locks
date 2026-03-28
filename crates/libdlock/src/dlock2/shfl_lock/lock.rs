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
    sync::atomic::{AtomicPtr, AtomicU32, Ordering},
};

use lock_api::{GuardSend, RawMutex};
use thread_local::ThreadLocal;

use super::node::{QNode, STATUS_LOCKED, STATUS_WAIT};

/// Threshold mask for `keep_lock_local()` — controls shuffle probability.
const THRESHOLD: u32 = 0xffff;

/// Shuffle quota PRNG modulus (must be power of 2).
const UNLOCK_COUNT_THRESHOLD: u32 = 1024;

/// Value representing "locked" in the `val` word (bit 0).
/// Used by the fast-path CAS on the full u32 word.
const LOCKED_VAL: u32 = 1;

/// Bit mask for the no_stealing flag (bit 8) within `val`.
/// Matches C: `_AQS_NOSTEAL_VAL = 1U << 8 = 0x100`.
const NOSTEAL_VAL: u32 = 1 << 8;

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
    let cores_per_node = cpu_number / numa_nodes;
    if cores_per_node == 0 {
        return 0;
    }
    (core / cores_per_node) as i32
}

struct Topology {
    cpu_count: u32,
    numa_nodes: u32,
}

static TOPOLOGY: std::sync::LazyLock<Topology> = std::sync::LazyLock::new(|| {
    // Use num_cpus::get() which returns the system-wide CPU count
    // regardless of thread affinity.  This matches C's
    // sysconf(_SC_NPROCESSORS_ONLN).
    //
    // std::thread::available_parallelism() is WRONG here because it
    // respects the calling thread's affinity mask.  If the LazyLock is
    // first triggered by a pinned benchmark thread, it could return 1
    // while numa_nodes is 2, causing cpu_count / numa_nodes = 0 and a
    // divide-by-zero in current_numa_node().
    let cpu_count = (num_cpus::get() as u32).max(1);

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
// RawShflLock
// ---------------------------------------------------------------------------

/// Raw ShflLock (AQS) implementing [`lock_api::RawMutex`].
///
/// The `val` field mirrors the C `aqs_mutex_t` union layout (little-endian):
///   bits  0..7  = locked byte (0 = unlocked, 1 = locked)
///   bits  8..15 = no_stealing byte (0 = stealing enabled, 0x100 = disabled)
///   bits 16..31 = reserved (always 0)
#[derive(Debug)]
pub struct RawShflLock {
    tail: AtomicPtr<QNode>,
    /// Combined lock state word.  Matches C `aqs_mutex_t.val`.
    val: AtomicU32,
    /// Per-thread queue nodes.
    local_node: ThreadLocal<QNode>,
}

// SAFETY: The lock provides synchronisation for all shared state.
unsafe impl Send for RawShflLock {}
unsafe impl Sync for RawShflLock {}

impl RawShflLock {
    /// Byte-level view of the locked byte (offset 0 of `val` on little-endian).
    /// Matches C union accessor `lock->locked`.
    ///
    /// SAFETY: Caller must ensure `self` is a valid reference.
    #[inline]
    fn locked_byte(&self) -> &std::sync::atomic::AtomicU8 {
        unsafe { &*((&self.val as *const AtomicU32).cast::<std::sync::atomic::AtomicU8>()) }
    }

    /// Clear the no_stealing flag.  Atomic RMW on u32, matching
    /// C: `atomic_andnot(_AQS_NOSTEAL_VAL, &lock->val)`.
    #[inline]
    fn enable_stealing(&self) {
        self.val.fetch_and(!NOSTEAL_VAL, Ordering::Release);
    }

    /// Set the no_stealing flag.  Atomic RMW on u32, matching
    /// C: `atomic_fetch_or_acquire(_AQS_NOSTEAL_VAL, &lock->val)`.
    #[inline]
    fn disable_stealing(&self) {
        self.val.fetch_or(NOSTEAL_VAL, Ordering::Acquire);
    }

    /// Pass shuffle leadership to `sleader`, recording `qend` as the
    /// last-visited position.  Matches C `set_sleader()`.
    ///
    /// SAFETY: `sleader` must be a valid, live queue node.
    #[inline]
    unsafe fn set_sleader(sleader: *mut QNode, qend: *mut QNode) {
        (*sleader).sleader.store(true, Ordering::Release);
        if qend != sleader {
            (*sleader).last_visited.store(qend, Ordering::Release);
        }
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

        let mut last: *mut QNode = node;
        let mut one_shuffle = false;

        if curr_locked_count == 0 {
            curr_locked_count += 1;
            (*node).wcount.store(curr_locked_count, Ordering::Relaxed);
        }

        // Clear our own shuffle leader flag.
        (*node).sleader.store(false, Ordering::Relaxed);

        // Probabilistic exit: occasionally stop shuffling to let
        // remote-socket threads through (starvation prevention).
        // Matches C: sleader = READ_ONCE(node->next); goto out;
        let (sleader, qend) = if !keep_lock_local() {
            ((*node).next.load(Ordering::Acquire), null_mut())
        } else {
            loop {
                let curr = (*prev).next.load(Ordering::Acquire);
                // Compiler barrier matching C: barrier() after READ_ONCE(prev->next).
                std::sync::atomic::compiler_fence(Ordering::SeqCst);

                if curr.is_null() {
                    break (last, prev);
                }

                if curr == self.tail.load(Ordering::Acquire) {
                    break (last, prev);
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
                            break (last, prev);
                        }

                        (*curr).wcount.store(curr_locked_count, Ordering::Relaxed);
                        // Pointer surgery matching C plain writes:
                        //   prev->next = next;
                        //   curr->next = last->next;
                        //   last->next = curr;
                        (*prev).next.store(next, Ordering::Relaxed);
                        (*curr)
                            .next
                            .store((*last).next.load(Ordering::Relaxed), Ordering::Relaxed);
                        (*last).next.store(curr, Ordering::Relaxed);
                        last = curr;
                        one_shuffle = true;
                    }
                } else {
                    prev = curr;
                }

                // Early exit: lock became available or we were promoted.
                let lock_ready = self.locked_byte().load(Ordering::Acquire) == 0;
                if one_shuffle
                    && ((is_next_waiter && lock_ready)
                        || (!is_next_waiter && (*node).lstatus.load(Ordering::Acquire) != 0))
                {
                    break (last, prev);
                }
            }
        };

        // out: pass shuffle leadership.
        if !sleader.is_null() {
            Self::set_sleader(sleader, qend);
        }
    }
}

unsafe impl RawMutex for RawShflLock {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = RawShflLock {
        tail: AtomicPtr::new(null_mut()),
        val: AtomicU32::new(0),
        local_node: ThreadLocal::new(),
    };

    type GuardMarker = GuardSend;

    fn lock(&self) {
        // Force topology init.
        let _ = &*TOPOLOGY;

        let node = self.local_node.get_or(QNode::new);
        let node_ptr: *mut QNode = node as *const QNode as *mut QNode;

        // --- Fast path: uncontended CAS on val ---
        // CAS the full val word from 0 (unlocked, stealing enabled) to 1
        // (locked).  Succeeds only when locked==0 AND no_stealing==0.
        // Matches C: smp_cas(&lock->locked_no_stealing, 0, 1)
        if self
            .val
            .compare_exchange_weak(0, LOCKED_VAL, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            return;
        }

        // --- Slow path: MCS-style enqueue ---
        unsafe {
            (*node_ptr).next.store(null_mut(), Ordering::Relaxed);
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
        // Matches C: READ_ONCE(lock->locked) — byte-level read.
        loop {
            if self.locked_byte().load(Ordering::Acquire) == 0 {
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
        // Matches C: smp_cas(&lock->locked, 0, 1) — byte-level CAS.
        loop {
            if self
                .locked_byte()
                .compare_exchange_weak(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }

            while self.locked_byte().load(Ordering::Acquire) != 0 {
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
        // Matches C: smp_cas(&lock->locked, 0, 1) — byte-level CAS.
        self.locked_byte()
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    unsafe fn unlock(&self) {
        // Store zero to the locked byte, matching C: WRITE_ONCE(lock->locked, 0).
        // This is a plain byte store (not an RMW), preserving no_stealing in byte 1.
        self.locked_byte().store(0, Ordering::Release);
    }
}
