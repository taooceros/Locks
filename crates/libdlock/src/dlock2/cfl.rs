//! Local CFL-family adaptation of fairnumas; published CFL fidelity is unverified.
//!
//! The queue-shuffling algorithm is ported from the repository's adapted C
//! implementation (`c/cfl/cfl.c`), NOT a pinned copy of an author release.
//!
//! Memory access strategy (matching C semantics without UB):
//!
//! - **QNode cross-thread fields** (`next`, `lstatus`, `sleader`, `wcount`,
//!   `last_visited`): `Atomic*` types. On x86, `load(Relaxed)` compiles to
//!   the same `mov` as C's `READ_ONCE`, and `store(Release)` compiles to
//!   the same `mov` as C's `WRITE_ONCE`. Zero performance cost vs volatile.
//!
//! - **QNode thread-local fields** (`nid`, `cid`, `runtime`): Plain types.
//!   Written before enqueue (happens-before via release on `prev->next`)
//!   or only by the lock holder. No cross-thread data race.
//!
//! - **Global vLHT arrays** (`RUNTIME_CHECKER_CORE/NODE`): `AtomicU64`.
//!   Relaxed fetch-add is required because different CFL handles may update
//!   the same process-global entry concurrently. CPU and node indices are
//!   bounds-checked and unsupported topologies fail-stop.
//!
//! - **Lock state** (`tail`, `val`): `AtomicPtr`/`AtomicU32` — need real
//!   CAS / swap / fetch_or / fetch_and.

use std::{
    cell::UnsafeCell,
    ptr::null_mut,
    sync::atomic::{
        AtomicBool, AtomicI32, AtomicPtr, AtomicU16, AtomicU32, AtomicU64, AtomicU8, Ordering,
    },
};

use lock_api::{GuardNoSend, RawMutex};
use thread_local::ThreadLocal;

// ====================================================================
// Constants (from cfl.h)
// ====================================================================

// C: CFL_STATUS_WAIT  0
const STATUS_WAIT: u8 = 0;
// C: CFL_STATUS_LOCKED 1
const STATUS_LOCKED: u8 = 1;

/// Bit mask for no_stealing flag (bit 8) within val.
/// C: CFL_NOSTEAL_VAL = 1U << (CFL_LOCKED_OFFSET + CFL_LOCKED_BITS) = 0x100
const NOSTEAL_VAL: u32 = 1 << 8;

/// C: #define THRESHOLD (0xffff)
const THRESHOLD: u32 = 0xffff;

/// C: #define UNLOCK_COUNT_THRESHOLD 1024
const UNLOCK_COUNT_THRESHOLD: u32 = 1024;

// NUMA placement correction to the local proxy: author fairnumas uses
// AUX CPU ID modulo node count, which is wrong for contiguous socket IDs.
// Cache the actual sysfs CPU->NUMA-node mapping once. The queue algorithm
// and vLHT's fixed array sizes are unchanged; unsupported IDs fail-stop.
struct Topology {
    nodes: u32,
    cpu_node: [u8; 256],
}

static TOPOLOGY: std::sync::LazyLock<Topology> = std::sync::LazyLock::new(detect_topology);

#[cold]
fn detect_topology() -> Topology {
    let root = std::path::Path::new("/sys/devices/system");
    let mut physical_nodes = [0usize; 16];
    let mut node_count = 0;
    for node in 0..256 {
        if root.join(format!("node/node{node}")).exists() {
            if node_count == physical_nodes.len() {
                std::process::abort();
            }
            physical_nodes[node_count] = node;
            node_count += 1;
        }
    }
    let mut cpu_node = [u8::MAX; 256];
    if node_count == 0 {
        // UMA kernels may not expose /sys/devices/system/node.
        cpu_node.fill(0);
    } else {
        for (slot, node) in physical_nodes[..node_count].iter().enumerate() {
            for cpu in 0..cpu_node.len() {
                if root.join(format!("cpu/cpu{cpu}/node{node}")).exists() {
                    if cpu_node[cpu] != u8::MAX {
                        std::process::abort();
                    }
                    cpu_node[cpu] = slot as u8;
                }
            }
        }
    }
    Topology {
        nodes: node_count.max(1) as u32,
        cpu_node,
    }
}

#[inline]
fn numa_nodes() -> u32 {
    TOPOLOGY.nodes
}

// ====================================================================
// Per-thread ID (C lines 49-56)
// Not needed in Rust — use thread::current().id() to seed PRNG.
// ====================================================================

// ====================================================================
// Globals (adapted fairnumas; deliberately shared across all CFL handles)
// ====================================================================

// C: unsigned long runtime_checker_core[256];
// Reads are relaxed; writes use fetch_add to avoid lost updates across locks.
#[allow(clippy::declare_interior_mutable_const)]
static RUNTIME_CHECKER_CORE: [AtomicU64; 256] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; 256]
};

// C: unsigned long runtime_checker_node[16];
#[allow(clippy::declare_interior_mutable_const)]
static RUNTIME_CHECKER_NODE: [AtomicU64; 16] = {
    const ZERO: AtomicU64 = AtomicU64::new(0);
    [ZERO; 16]
};

// C: int allowed_node;
static ALLOWED_NODE: AtomicI32 = AtomicI32::new(0);

/// Checked access to `RUNTIME_CHECKER_CORE[idx]`; unsupported CPU aborts.
#[inline(always)]
unsafe fn rt_core(idx: usize) -> &'static AtomicU64 {
    RUNTIME_CHECKER_CORE
        .get(idx)
        .unwrap_or_else(|| std::process::abort())
}

/// Checked access to `RUNTIME_CHECKER_NODE[idx]`; unsupported node aborts.
#[inline(always)]
unsafe fn rt_node(idx: usize) -> &'static AtomicU64 {
    RUNTIME_CHECKER_NODE
        .get(idx)
        .unwrap_or_else(|| std::process::abort())
}

// ====================================================================
// NUMA helpers (local portability correction, not original CPU modulo N)
// ====================================================================

/// Actual sysfs NUMA mapping for the CPU sampled by RDTSCP, not CPU parity.
#[inline]
fn node_for_cpu(core: i32) -> i32 {
    let node = TOPOLOGY
        .cpu_node
        .get(core as usize)
        .copied()
        .unwrap_or_else(|| std::process::abort());
    if node == u8::MAX {
        std::process::abort();
    }
    i32::from(node)
}

/// C: current_numa_core() — raw core ID
#[inline]
fn current_numa_core() -> i32 {
    // C: unsigned long a, d, c; int core;
    // C: __asm__ volatile("rdtscp" : "=a"(a), "=d"(d), "=c"(c));
    // C: core = c & 0xFFF;
    let core: u32;
    unsafe {
        let mut aux: u32 = 0;
        core::arch::x86_64::__rdtscp(&mut aux);
        core = aux & 0xFFF;
        if core >= RUNTIME_CHECKER_CORE.len() as u32 {
            std::process::abort();
        }
    }
    // C: return core;
    core as i32
}

// ====================================================================
// Shuffle quota PRNG (C lines 91-112, from fairnumas.c, verbatim)
// ====================================================================

std::thread_local! {
    // C: static __thread uint32_t rv = 0;
    static XOR_RV: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// C: xor_random()
#[inline]
fn xor_random() -> u32 {
    XOR_RV.with(|cell| {
        let mut v = cell.get();
        // C: if (rv == 0) rv = cfl_thread_id + 1;
        if v == 0 {
            v = std::thread::current().id().as_u64().get() as u32;
            if v == 0 {
                v = 1;
            }
        }
        // C: v ^= v << 6; v ^= (uint32_t)(v) >> 21; v ^= v << 7;
        v ^= v << 6;
        v ^= v >> 21;
        v ^= v << 7;
        // C: rv = v;
        cell.set(v);
        // C: return v & (UNLOCK_COUNT_THRESHOLD - 1);
        v & (UNLOCK_COUNT_THRESHOLD - 1)
    })
}

/// C: keep_lock_local()
#[inline]
fn keep_lock_local() -> bool {
    // C: return xor_random() & THRESHOLD;
    (xor_random() & THRESHOLD) != 0
}

// ====================================================================
// TSC reading
// ====================================================================

/// C: cfl_rdtsc() — uses `rdtsc` (NOT `rdtscp`) to match C.
/// `rdtsc` is ~10 cycles cheaper than `rdtscp` since it's non-serializing.
#[inline]
fn cfl_rdtsc() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

// ====================================================================
// QNode — per-thread queue node (C cfl_node_t)
//
// Cross-thread fields use atomics (zero-cost on x86 for loads/stores).
// Thread-local fields (nid, cid, runtime) use plain types.
// ====================================================================

/// C: typedef struct cfl_node { ... } cfl_node_t;
/// Layout must match C for cache-line locality of hot fields.
#[repr(C, align(128))]
struct QNode {
    // C: struct cfl_node *next;  — cross-thread: READ_ONCE/WRITE_ONCE + plain
    next: AtomicPtr<QNode>,
    // C: uint8_t lstatus;  — cross-thread: READ_ONCE/WRITE_ONCE
    lstatus: AtomicU8,
    // C: uint8_t sleader;  — cross-thread: WRITE_ONCE by shuffler, READ_ONCE by owner
    sleader: AtomicBool,
    // C: uint16_t wcount;  — cross-thread: WRITE_ONCE by shuffler, plain read by owner
    wcount: AtomicU16,
    // C: int nid;  — thread-local: set before enqueue, read after happens-before
    nid: i32,
    // C: int cid;  — thread-local: set before enqueue, read after happens-before
    cid: i32,
    // C: struct cfl_node *last_visited;  — cross-thread: WRITE_ONCE/READ_ONCE
    last_visited: AtomicPtr<QNode>,
    // C: unsigned long runtime;  — thread-local: only lock holder reads/writes
    runtime: u64,
}

impl QNode {
    const fn new() -> Self {
        QNode {
            next: AtomicPtr::new(null_mut()),
            lstatus: AtomicU8::new(STATUS_WAIT),
            sleader: AtomicBool::new(false),
            wcount: AtomicU16::new(0),
            nid: 0,
            cid: 0,
            last_visited: AtomicPtr::new(null_mut()),
            runtime: 0,
        }
    }
}

// SAFETY: Cross-thread fields are atomic. Plain fields (nid, cid, runtime)
// are written before enqueue (happens-before via release on prev->next store)
// or only by the lock holder.
unsafe impl Send for QNode {}
unsafe impl Sync for QNode {}

// ====================================================================
// RawCflLock (C cfl_mutex_t)
// ====================================================================

/// CFL lock implementing [`lock_api::RawMutex`].
///
/// Local adapted fairnumas port with NUMA-aware vLHT-based queue shuffling.
/// Provenance as a faithful published CFL implementation remains unverified.
#[derive(Debug)]
pub struct RawCflLock {
    // C: struct cfl_node *tail;
    tail: AtomicPtr<QNode>,
    // C: union { uint32_t val; struct { uint8_t locked; uint8_t no_stealing; }; ... };
    val: AtomicU32,
    /// Per-thread queue nodes (wrapped in UnsafeCell for mutable access to plain fields).
    local_node: ThreadLocal<UnsafeCell<QNode>>,
}

// SAFETY: The lock serialises all concurrent accesses.
unsafe impl Send for RawCflLock {}
unsafe impl Sync for RawCflLock {}

impl RawCflLock {
    // The lock word is always accessed as a single AtomicU32. A byte-sized
    // atomic overlay on an AtomicU32 is undefined behavior in Rust.
    #[inline]
    fn locked(&self) -> bool {
        self.val.load(Ordering::Acquire) & 0xff != 0
    }
    // ================================================================
    // Stealing control helpers (C lines 117-130)
    // ================================================================

    /// C: enable_stealing(lock) — atomic_andnot(CFL_NOSTEAL_VAL, &lock->val)
    #[inline]
    fn enable_stealing(&self) {
        self.val.fetch_and(!NOSTEAL_VAL, Ordering::Release);
    }

    /// C: disable_stealing(lock) — atomic_fetch_or_acquire(CFL_NOSTEAL_VAL, &lock->val)
    #[inline]
    fn disable_stealing(&self) {
        self.val.fetch_or(NOSTEAL_VAL, Ordering::Acquire);
    }

    // ================================================================
    // Shuffle leader helpers (C lines 135-150)
    // ================================================================

    /// C: set_sleader(node, qend)
    #[inline]
    unsafe fn set_sleader(node: *mut QNode, qend: *mut QNode) {
        // C: WRITE_ONCE(node->sleader, 1);
        (*node).sleader.store(true, Ordering::Release);
        // C: if (qend != node) WRITE_ONCE(node->last_visited, qend);
        if qend != node {
            (*node).last_visited.store(qend, Ordering::Release);
        }
    }

    /// C: set_waitcount(node, count)
    #[inline]
    unsafe fn set_waitcount(node: *mut QNode, count: u16) {
        // C: WRITE_ONCE(node->wcount, count);
        (*node).wcount.store(count, Ordering::Relaxed);
    }

    // ================================================================
    // need_switch() — NUMA fairness decision (C lines 155-181)
    // ================================================================

    /// C: need_switch() — find min-runtime NUMA node
    #[inline]
    fn need_switch() -> i32 {
        let numa_nodes = numa_nodes() as usize;
        // C: int i, minid;
        let mut minid: i32 = 0;
        // C: unsigned long max, min, threshold, value;
        let mut max: u64 = 0;
        // C: min = runtime_checker_node[0];
        let mut min: u64 = unsafe { rt_node(0) }.load(Ordering::Relaxed);
        let threshold: u64 = 100000;
        let mut value: u64;

        // C: for (i = 0; i < numa_nodes; i++)
        for i in 0..numa_nodes {
            // C: value = READ_ONCE(runtime_checker_node[i]);
            value = unsafe { rt_node(i) }.load(Ordering::Relaxed);
            // C: if (max < value) { max = value; }
            if max < value {
                max = value;
            }
            // C: if (min > value) { min = value; minid = i; }
            if min > value {
                min = value;
                minid = i as i32;
            }
        }

        // C: if (max - min <= threshold)
        if max.wrapping_sub(min) <= threshold {
            // C: WRITE_ONCE(allowed_node, 100); return 100;
            ALLOWED_NODE.store(100, Ordering::Relaxed);
            100
        } else {
            // C: WRITE_ONCE(allowed_node, minid); return minid;
            ALLOWED_NODE.store(minid, Ordering::Relaxed);
            minid
        }
    }

    // ================================================================
    // shuffle_waiters() — core CFL queue reordering
    // (C lines 187-306, from fairnumas.c)
    // ================================================================
    unsafe fn shuffle_waiters(&self, node: *mut QNode, is_next_waiter: bool) {
        let mut curr_locked_count: u16 = (*node).wcount.load(Ordering::Relaxed);
        let mut one_shuffle = false;

        // C: prev, last, stand all start at node.
        // (C loads last_visited into prev first, but unconditionally overwrites it.)
        let mut prev: *mut QNode = node;
        let mut last: *mut QNode = node;
        let stand: *mut QNode = node;

        // C: if (curr_locked_count == 0) set_waitcount(node, ++curr_locked_count);
        if curr_locked_count == 0 {
            curr_locked_count += 1;
            Self::set_waitcount(node, curr_locked_count);
        }

        // C: clear_sleader(node);
        (*node).sleader.store(false, Ordering::Relaxed);

        // C: if (!keep_lock_local()) { }
        // PRNG side-effect only (starvation prevention); result unused.
        let _ = keep_lock_local();

        // C: nid = need_switch();
        let mut nid: i32 = Self::need_switch();
        // C: if (nid == 100) nid = node->nid;
        if nid == 100 {
            nid = (*node).nid;
        }

        // C: standard = READ_ONCE(runtime_checker_node[nid]) / 16;
        let standard = rt_node(nid as usize).load(Ordering::Relaxed) / 16;

        // C: for (;;) { ... }
        // Loop returns (sleader, qend) via break to avoid dead-store warnings.
        let (sleader, qend) = 'main_loop: loop {
            // C: curr = READ_ONCE(prev->next);
            let curr = (*prev).next.load(Ordering::Acquire);

            // C: barrier();
            std::sync::atomic::compiler_fence(Ordering::SeqCst);

            // C: if (!curr) { sleader = last; qend = prev; break; }
            if curr.is_null() {
                break 'main_loop (last, prev);
            }

            // C: if (curr == READ_ONCE(lock->tail)) { ... break; }
            if curr == self.tail.load(Ordering::Acquire) {
                break 'main_loop (last, prev);
            }

            // Labeled block for goto check → break 'numa
            'numa: {
                // C: if (curr->nid == nid)
                if (*curr).nid == nid {
                    // C: if (prev == node && prev->nid == nid)
                    if prev == node && (*prev).nid == nid {
                        Self::set_waitcount(curr, curr_locked_count);
                        last = curr;
                        prev = curr;
                        one_shuffle = true;
                    } else {
                        // C: next = READ_ONCE(curr->next);
                        let next = (*curr).next.load(Ordering::Acquire);
                        // C: if (!next) { ... goto out; }
                        if next.is_null() {
                            break 'main_loop (last, prev);
                        }

                        // C: if (runtime_checker_core[curr->cid] >= standard)
                        //        { prev = curr; goto check; }
                        if rt_core((*curr).cid as usize).load(Ordering::Relaxed) >= standard {
                            prev = curr;
                            break 'numa; // goto check
                        }

                        // C: iter = stand;
                        let mut iter = stand;

                        // C: while (iter->next && ...) { iter = iter->next; barrier(); }
                        loop {
                            let iter_next = (*iter).next.load(Ordering::Relaxed);
                            if iter_next.is_null() {
                                break;
                            }
                            if (*iter_next).nid != (*curr).nid {
                                break;
                            }
                            if rt_core((*curr).cid as usize).load(Ordering::Relaxed)
                                <= rt_core((*iter_next).cid as usize).load(Ordering::Relaxed)
                            {
                                break;
                            }
                            if iter == last {
                                break;
                            }
                            iter = iter_next;
                            std::sync::atomic::compiler_fence(Ordering::SeqCst);
                        }

                        Self::set_waitcount(curr, curr_locked_count);

                        if iter != prev {
                            (*prev).next.store(next, Ordering::Relaxed);
                            (*curr)
                                .next
                                .store((*iter).next.load(Ordering::Relaxed), Ordering::Relaxed);
                            (*iter).next.store(curr, Ordering::Relaxed);
                        } else {
                            prev = curr;
                        }

                        if iter == last {
                            last = curr;
                        }
                        one_shuffle = true;
                    }
                } else {
                    prev = curr;
                }
            } // end 'numa block

            // check:
            let lock_ready = !self.locked();
            if one_shuffle
                && ((is_next_waiter && lock_ready)
                    || (!is_next_waiter && (*node).lstatus.load(Ordering::Acquire) != 0))
            {
                break 'main_loop (last, prev);
            }
        }; // end 'main_loop

        // out:
        if !sleader.is_null() {
            Self::set_sleader(sleader, qend);
        }
    }
}

// ====================================================================
// impl RawMutex
// ====================================================================

unsafe impl RawMutex for RawCflLock {
    #[allow(clippy::declare_interior_mutable_const)]
    const INIT: Self = RawCflLock {
        tail: AtomicPtr::new(null_mut()),
        val: AtomicU32::new(0),
        local_node: ThreadLocal::new(),
    };

    type GuardMarker = GuardNoSend;

    // ================================================================
    // lock() — maps to __cfl_lock() (C lines 312-430)
    // ================================================================
    fn lock(&self) {
        let cell = self.local_node.get_or(|| UnsafeCell::new(QNode::new()));
        let me: *mut QNode = cell.get();

        unsafe {
            // C: me->cid = current_numa_core();
            (*me).cid = current_numa_core();
            (*me).nid = node_for_cpu((*me).cid);
            // C: me->runtime = 0;
            (*me).runtime = 0;

            // Labeled block for goto release → break 'body
            'body: {
                // C: if (allowed_node == 100 ||
                //        (me->nid == allowed_node &&
                //         runtime_checker_core[me->cid] <
                //             READ_ONCE(runtime_checker_node[me->nid]) / 16))
                let allowed = ALLOWED_NODE.load(Ordering::Relaxed);
                let nid = (*me).nid;
                let cid = (*me).cid;
                if allowed == 100
                    || (nid == allowed
                        && rt_core(cid as usize).load(Ordering::Relaxed)
                            < rt_node(nid as usize).load(Ordering::Relaxed) / 16)
                {
                    // C: if (smp_cas(&impl->locked_no_stealing, 0, 1) == 0) goto release;
                    if self
                        .val
                        .compare_exchange_weak(0, 1, Ordering::AcqRel, Ordering::Relaxed)
                        .is_ok()
                    {
                        break 'body; // goto release
                    }
                }

                // The node is private until its tail publication; reset its
                // atomics individually instead of writing over their storage.
                (*me).lstatus.store(STATUS_WAIT, Ordering::Relaxed);
                (*me).sleader.store(false, Ordering::Relaxed);
                (*me).wcount.store(0, Ordering::Relaxed);
                // C: me->next = NULL;
                (*me).next.store(null_mut(), Ordering::Relaxed);
                // C: me->last_visited = NULL;
                (*me).last_visited.store(null_mut(), Ordering::Relaxed);

                // C: /* Publish the updated tail. */
                // C: prev = smp_swap(&impl->tail, me);
                let prev = self.tail.swap(me, Ordering::AcqRel);

                // C: if (prev)
                if !prev.is_null() {
                    // C: WRITE_ONCE(prev->next, me);
                    (*prev).next.store(me, Ordering::Release);

                    // C: for (;;)
                    loop {
                        // C: if (READ_ONCE(me->lstatus) == CFL_STATUS_LOCKED) break;
                        if (*me).lstatus.load(Ordering::Acquire) == STATUS_LOCKED {
                            break;
                        }

                        // C: if (READ_ONCE(me->sleader))
                        //        shuffle_waiters(impl, me, 0);
                        if (*me).sleader.load(Ordering::Acquire) {
                            self.shuffle_waiters(me, false);
                        }

                        // C: CPU_PAUSE();
                        core::hint::spin_loop();
                    }
                } else {
                    // C: else disable_stealing(impl);
                    self.disable_stealing();
                }

                // C: for (;;)
                loop {
                    // C: if (!READ_ONCE(impl->locked)) break;
                    if !self.locked() {
                        break;
                    }

                    // C: wcount = me->wcount;
                    let wcount = (*me).wcount.load(Ordering::Relaxed);
                    // C: if (!wcount || (wcount && me->sleader))
                    if wcount == 0 || (*me).sleader.load(Ordering::Relaxed) {
                        self.shuffle_waiters(me, true);
                    }
                }

                // C: for (;;)
                loop {
                    let state = self.val.load(Ordering::Acquire);
                    if state & 0xff == 0
                        && self
                            .val
                            .compare_exchange_weak(
                                state,
                                state | 1,
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            )
                            .is_ok()
                    {
                        break;
                    }
                    while self.locked() {
                        core::hint::spin_loop();
                    }
                }

                // C: if (!READ_ONCE(me->next))
                if (*me).next.load(Ordering::Acquire).is_null() {
                    // C: if (smp_cas(&impl->tail, me, NULL) == me)
                    if self
                        .tail
                        .compare_exchange(me, null_mut(), Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        // C: enable_stealing(impl); goto release;
                        self.enable_stealing();
                        break 'body; // goto release
                    }

                    // C: while (!READ_ONCE(me->next)) CPU_PAUSE();
                    while (*me).next.load(Ordering::Acquire).is_null() {
                        core::hint::spin_loop();
                    }
                }

                // C: WRITE_ONCE(me->next->lstatus, 1);
                let next = (*me).next.load(Ordering::Acquire);
                (*next).lstatus.store(STATUS_LOCKED, Ordering::Release);
            } // end 'body

            // release:
            // C: barrier();
            std::sync::atomic::compiler_fence(Ordering::SeqCst);
            // C: me->runtime = cfl_rdtsc();
            (*me).runtime = cfl_rdtsc();
            // C: barrier();
            std::sync::atomic::compiler_fence(Ordering::SeqCst);
        }
    }

    fn try_lock(&self) -> bool {
        // Fast-path CAS matching __cfl_lock fast path.
        // CAS val: locked=0, no_stealing=0 → locked=1.
        // Complete all potentially panicking TLS/topology setup before CAS,
        // so failure cannot leave the lock permanently acquired.
        let cell = self.local_node.get_or(|| UnsafeCell::new(QNode::new()));
        let me: *mut QNode = cell.get();
        let cid = current_numa_core();
        let nid = node_for_cpu(cid);
        if self
            .val
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            unsafe {
                (*me).cid = cid;
                (*me).nid = nid;
                (*me).runtime = cfl_rdtsc();
            }
            true
        } else {
            false
        }
    }

    // ================================================================
    // unlock() — maps to __cfl_unlock() (C lines 436-446)
    // ================================================================
    unsafe fn unlock(&self) {
        let cell = self.local_node.get_or(|| UnsafeCell::new(QNode::new()));
        let me: *mut QNode = cell.get();

        // C: unsigned long cslength;
        // C: if (me->runtime != 0)
        let runtime = (*me).runtime;
        if runtime != 0 {
            // C: cslength = cfl_rdtsc() - me->runtime;
            let cslength = cfl_rdtsc().wrapping_sub(runtime);
            // C: runtime_checker_core[me->cid] += cslength;
            // Across independent CFL handles, different owners may update the
            // same process-wide vLHT slot concurrently.
            let idx_core = (*me).cid as usize;
            rt_core(idx_core).fetch_add(cslength, Ordering::Relaxed);
            // C: runtime_checker_node[me->nid] += cslength;
            let idx_node = (*me).nid as usize;
            rt_node(idx_node).fetch_add(cslength, Ordering::Relaxed);
        }

        // C: WRITE_ONCE(impl->locked, 0);
        self.val.fetch_and(!0xff, Ordering::Release);
    }
}
