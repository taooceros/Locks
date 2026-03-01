# Research Plan: Usage-Fair Delegation Locks

## Working Title

**"Usage-Fair Delegation Locks: Combining Throughput Without Sacrificing Scheduler Cooperation"**

Alternative:
*"Fair Combining: Scheduler-Cooperative Delegation Locks via Usage-Aware Combiner Scheduling"*

---

## 1. Problem Statement

Delegation-style locks (Flat Combining, CCSynch, DSMSynch, RCL, FFWD) achieve
superior throughput by having a single *combiner* thread execute critical
sections on behalf of waiting threads. This eliminates shared-data migration
across cores and amortizes synchronization overhead.

However, delegation locks suffer from two fairness problems that hinder
practical adoption:

**Problem A — Usage Unfairness.** When threads have heterogeneous critical
section lengths, delegation locks distribute lock-holding time proportionally
to critical section size rather than equally across threads. A thread with a
30ns critical section gets 3x the lock usage of a thread with a 10ns critical
section, even though both are "fairly" served in acquisition order. This
constitutes *scheduler subversion* — the lock policy overrides the OS
scheduler's intent to give equal CPU time to equal-priority threads.

**Problem B — Combiner Response-Time Penalty.** The combiner thread bears a
disproportionate latency burden. In Flat Combining, the combiner's response
time equals the *sum* of all critical sections in the current pass. Under 32
threads, we observe that 2 threads spend >80% of execution time as combiners.
This creates severe tail-latency asymmetry.

---

## 2. Thesis

We can systematically retrofit usage-fairness into delegation-style locks by
treating the combiner as a *user-level scheduler* that manages lock-holding
time budgets per thread — analogous to how the Linux CFS manages CPU time via
virtual runtime. We demonstrate two concrete strategies:

1. **Banning** — threads that consume disproportionate lock time are temporarily
   excluded, inspired by U-SCL's lock-slice accounting.
2. **Priority-based combining** — replacing the combiner's linear traversal with
   a priority structure (skip list, binary heap, B-tree) ordered by cumulative
   lock usage, ensuring threads with less accumulated usage are served first.

Crucially, delegation *decouples fairness from data locality*: since the
shared data stays in the combiner's L1 cache regardless of which waiter is
served, reordering for fairness does not incur the cross-core cache
migration penalty that makes traditional fair locks slower than unfair ones.
These strategies preserve the throughput advantage of delegation while
achieving usage-fairness and bounded tail latency.

---

## 3. Contributions

1. **Systematic fairness framework for delegation locks.** We formalize
   *usage-fairness* in the context of delegation locks and show that
   acquisition-fairness (FIFO ordering in CCSynch) is insufficient.

2. **Fair delegation lock variants.** We design and implement:
   - FC-Ban: Flat Combining with TSC-based banning
   - CC-Ban: CCSynch with waiter-side banning
   - FC-PQ: Flat Combining with priority queue (BinaryHeap / BTree)
   - FC-SL: Flat Combining with concurrent skip-list ordering

3. **Fairness without the performance tax.** We show that delegation
   breaks the traditional fairness-performance tradeoff: because shared
   data stays in the combiner's L1 regardless of serving order, fair
   delegation (FC-PQ) approaches the throughput of unfair delegation (FC),
   while traditional fair locks (CFL, MCS) pay a significant throughput
   penalty relative to unfair alternatives (TAS). We validate this
   experimentally across thread counts and CS heterogeneity levels.

4. **Combiner response-time analysis.** We characterize the combiner penalty
   across thread counts and show it worsens super-linearly with concurrency.
   We propose and evaluate *tail-as-combiner* selection to eliminate the
   penalty for the combiner thread.

5. **Comprehensive evaluation.** Micro-benchmarks across contention levels and
   thread counts (2-64+), plus end-to-end application benchmarks demonstrating
   real-world impact of usage-fairness.

---

## 4. Positioning Against Prior Work

### 4.1 Summary Table

| Work | What it does | Gap we address |
|------|-------------|----------------|
| Flat Combining (Hendler et al., SPAA'10) | Delegation via thread-list combining | No usage-fairness |
| CCSynch/DSMSynch (Fatourou & Kallimanis, PPoPP'12) | FIFO job-queue combining | FIFO ≠ usage-fair |
| RCL (Lozi et al., ATC'12) | Dedicated server thread | No fairness mechanism |
| FFWD (Roghanchi et al., SOSP'17) | Fast delegation via dedicated core | No fairness mechanism |
| U-SCL (Patel et al., EuroSys'20) | Scheduler-cooperative lock-slices | Lock-slice degrades with long non-CS; not delegation-based |
| TCLocks (Gupta et al., OSDI'23) | Transparent delegation via RSP/RIP capture | Addresses API transparency, not fairness |
| CFL (Manglik & Kim, PPoPP'24) | Fair queue-based lock via waiter-side reordering | Off-path reordering, not delegation; O(N) remote cache misses |
| ShflLock (Kashyap et al., SOSP'19) | Queue shuffling for NUMA locality | Performance policy, not fairness; same mechanism as CFL |
| Syncord (Park et al., OSDI'22) | eBPF framework for dynamic kernel lock policies | Framework, not algorithm; validates banning over pure reordering |

### 4.2 Deep Distinctions

#### vs. CFL and ShflLock: Where Fairness Scheduling Happens

CFL and ShflLock share the same core mechanism: a designated waiter thread
reorders the MCS wait queue OFF the critical path. ShflLock reorders by
NUMA socket ID (a static property); CFL reorders by virtual Lock Hold Time
(vLHT, a dynamic fairness metric). CFL's contribution over ShflLock is the
policy (usage-fairness), the problem framing (lock-as-resource), and cgroup
integration — not a new mechanism.

**Our approach is architecturally different.** We schedule ON the critical
path, inside the combiner. This creates a fundamental tradeoff:

| Aspect | CFL/ShflLock (off-path, waiter-side) | Our work (on-path, combiner-side) |
|--------|--------------------------------------|-----------------------------------|
| **Where scheduling happens** | In the wait queue, by a waiter thread | Inside the combiner, during CS execution |
| **Cache behavior** | O(N) cross-core reads per LS pass (each qnode is on a remote core's cache) | L1-hot: priority queue/skip list lives in combiner's cache |
| **Scheduling granularity** | Reorder waiters before they acquire | Choose which waiter to serve next, per CS |
| **Data structure** | Doubly-linked queue with CAS-based swaps | Sequential heap/B-tree/skip list (no contention) |
| **Lock API** | Traditional acquire/release (mutex-compatible) | Delegation API (`lock(data) -> result`) |
| **Applicability** | Queue-based locks (MCS, CLH) only | Delegation locks (FC, CCSynch, DSMSynch) only |
| **Throughput model** | Shared-data migrates between cores on each handoff | Shared-data stays in combiner's L1 (no migration) |

The key insight: delegation locks already centralize execution at the
combiner, so the combiner is a *natural scheduling point*. Scheduling
decisions are made over L1-hot data structures with zero remote memory
access. CFL's Lock Scheduler must traverse N remote queue nodes (each
requiring a cross-core cache read + CAS), making its scheduling cost
O(N) cache misses per pass. Our combiner pops from a local heap in
O(log N) time with zero remote reads.

CFL is work-conserving (it only reorders among existing waiters, never
excludes). Our FC-PQ and FC-SL are also work-conserving (serve the
lowest-usage waiter among those present). FC-Ban and CC-Ban are NOT
work-conserving (banned threads are excluded even if the lock is idle).
This is a design choice, not a limitation — banning provides strict
enforcement that pure reordering cannot (see Syncord below).

**What CFL does that we don't (yet):** cgroup integration for hierarchical
weight support, and explicit NUMA-awareness via the `min_sid` policy.

#### vs. Syncord: Reordering Alone Cannot Enforce Strict Fairness

Syncord (OSDI'22) built an eBPF framework for dynamically patching kernel
lock policies. Their key empirical finding is directly relevant to us:

> *Reordering alone does not strictly enforce fairness.* SCL+ReorderBully
> had 1659 policy violations, while SCL+BackoffBully (which adds admission
> control via backoff) had only 538.

This validates our banning approach (FC-Ban, CC-Ban). Pure reordering (as
in CFL) can improve fairness probabilistically but cannot *guarantee* it —
a thread that arrives when the queue is empty gets served immediately
regardless of its accumulated usage. Admission control (banning/backoff)
is necessary for strict enforcement.

However, Syncord also shows the cost: backoff wastes CPU time. Our priority-
based variants (FC-PQ, FC-SL) offer a middle path — they are
work-conserving like reordering but achieve stronger fairness than CFL's
approach because the combiner has full visibility of all waiting threads
and their usage history, not just the queue order.

Syncord is a *framework*; we provide *concrete algorithms*. Their 10 API
hooks (should_reorder, skip_reorder, backoff, etc.) demonstrate the design
space; we explore specific, well-characterized points in that space.

#### vs. TCLocks: Fairness and Transparency Are Orthogonal

TCLocks (OSDI'23) solve a different problem: making delegation locks
transparent to existing mutex acquire/release code. They capture RSP/RIP
at lock acquisition, create ephemeral stacks, and replay the critical
section on the combiner's core. This eliminates the need for the delegation
API (`lock(data) -> result`) that our work requires.

The two contributions are *complementary*:
- **TCLocks** answer: "How do I use delegation without changing code?"
- **We** answer: "How do I make delegation fair?"
- **TCLocks + our work** would answer: "How do I get fair delegation
  transparently?" — the ideal end state.

TCLocks specifically note a *resource accounting limitation*: the combiner
thread receives CPU time credit for executing waiters' critical sections,
confusing the OS scheduler. This is directly related to our problem — the
combiner's effective CPU usage is inflated by the time it spends on behalf
of others. A fair delegation lock would need to coordinate with the OS
scheduler to charge CS time to the correct thread. TCLocks also mention
`current` macro issues (kernel thread identity) as a limitation, which
fair scheduling would exacerbate since the combiner acts on behalf of
many threads.

TCLocks' implementation uses a TAS+MCS combination with DSMSynch-style
combining, batch limits up to 1024, and CNA-inspired NUMA-aware queue
splitting. Their ~47ns context-switch overhead per waiter is the cost of
transparency. Our delegation API avoids this overhead entirely but
requires code modification.

**TCLocks' evaluation methodology** (OSDI'23) provides a useful template
for delegation lock papers: (1) micro-benchmarks stressing a single lock
(FxMark MRDM/MWRM, will-it-scale lock1/mmap1, 30 seconds each, pinned
threads); (2) a nano-benchmark (RCUHT hash table) for deep factor
analysis isolating each optimization's contribution; (3) real applications
(Psearchy, Metis) stressing kernel subsystems; (4) userspace validation
(LevelDB). Their factor analysis (Fig 5g: Base → +NUMA → +Prefetch →
+WWJump) is particularly effective at decomposing throughput gains.
Key differences from our evaluation: they measure **p99.99 latency**
(lock+CS+unlock combined) but do NOT split combiner vs waiter roles,
they do NOT measure fairness (no JFI, no per-thread hold-time), and
they do NOT test heterogeneous CS workloads. These gaps are our
unique contribution in evaluation methodology.

#### vs. U-SCL: Delegation Preserves the Scheduling Point

U-SCL (Patel et al., 2020) adds usage-fairness to traditional MCS locks
via lock-slices and banning. We adapted their banning formula
(`banned_until += cs * (total_weight / weight)`) for FC-Ban.

Key differences:
1. **U-SCL bans the thread holding the lock;** we ban threads waiting for
   delegation. In U-SCL, a thread must acquire, check its budget, and
   voluntarily release. In FC-Ban, the combiner skips banned nodes without
   the thread ever touching the lock — zero overhead for banned threads.
2. **U-SCL's lock-slice degrades with long non-CS.** If a thread's non-CS
   is 10x its CS, the lock-slice may expire entirely during non-CS, making
   the banning window cover both CS and non-CS time. Our per-CS penalty
   (`cs_time * N_waiting`) is proportional only to actual lock usage.
3. **U-SCL is non-work-conserving;** so is FC-Ban. But we also offer
   FC-PQ/FC-SL which are work-conserving. U-SCL has no work-conserving
   variant.
4. **Throughput model is fundamentally different.** U-SCL's shared data
   migrates between cores on every lock handoff. Our delegation keeps
   shared data in the combiner's L1. Under high contention with short CS,
   delegation throughput can be 5-10x higher than MCS.

### 4.3 Architectural Comparison: Scheduling Location

```
Traditional locks (CFL/ShflLock/U-SCL):
  Thread₁ ──acquire──► [wait queue] ──schedule──► [hold lock] ──execute CS──► release
                            ↑
                        Scheduling happens here
                        (waiter reorders queue, or thread self-bans)
                        Cache: O(N) remote reads (CFL) or self-only (U-SCL)

Delegation locks (our work):
  Thread₁ ──announce──► [publication list] ──► Combiner ──schedule+execute──► result
                                                   ↑
                                               Scheduling happens here
                                               (combiner picks next from PQ/SL)
                                               Cache: L1-hot (combiner owns the data)
```

The delegation model *collapses* scheduling and execution into one point
(the combiner). This is why combiner-side scheduling is natural and cheap:
the combiner already reads every waiter's request data, so reading their
usage counters costs nothing extra. In traditional locks, the scheduler
(CFL's LS thread) must separately access each waiter's metadata across
cores.

### 4.4 Delegation Breaks the Fairness-Performance Tradeoff

In traditional locks, fairness and performance are fundamentally at odds.
When thread A releases a lock and thread B (on a different core) acquires
it next, the shared data must migrate from A's L1 cache to B's. This
cross-core cache-line transfer is the dominant cost of fair handoff. An
unfair lock (e.g., TAS with backoff) lets the releasing thread re-acquire
immediately — the shared data stays in its L1, no migration needed. This
is why every traditional fair lock (MCS, CFL, ShflLock) pays a throughput
penalty compared to unfair alternatives. NUMA amplifies the gap: cross-
socket migration costs ~3x more than same-socket.

**Delegation eliminates this tradeoff.** In a delegation lock, the shared
data *never leaves the combiner's cache*, regardless of which waiter is
served. Whether the combiner executes thread A's CS or thread B's CS, the
shared data structure stays in the same L1 line. The cost of "switching"
from one waiter to another is reading the new waiter's input data — a
single cache miss for the input, not for the shared structure itself.

This means reordering which waiter gets served — which is exactly what
fairness requires — has **no impact on shared-data locality**:

```
Traditional lock (fairness costs cache migration):
  Thread A releases → shared data in A's L1
  Thread B acquires → shared data migrates A's L1 → B's L1  [EXPENSIVE]
  Fair ordering forces this migration on every handoff.

Delegation lock (fairness is free w.r.t. shared data):
  Combiner serves A → shared data in combiner's L1
  Combiner serves B → shared data still in combiner's L1   [FREE]
  Fair ordering only changes which input the combiner reads next.
```

The remaining costs of fairness in delegation are:
1. **Priority queue maintenance:** O(log N) per CS, but over L1-hot data
   (the heap lives in the combiner's cache).
2. **Reading the next waiter's input:** One cache miss per CS, but this
   cost is identical regardless of which waiter is chosen — fair or unfair
   ordering pays the same price.

Both costs are small compared to the cross-core shared-data migration that
traditional fair locks must pay on every handoff.

**NUMA caveat (potential reviewer concern):** Fair reordering may cause the
combiner to fetch waiter input data from remote NUMA nodes in non-linear
order, reducing hardware prefetcher effectiveness. However, the cost is
bounded: each waiter's input is typically 1-2 cache lines (function pointer
+ argument), while the shared data structure can be hundreds or thousands of
cache lines (e.g., counter-array at 512 KiB = 8192 cache lines). Even
worst-case remote-NUMA input fetches (1 miss × ~100ns NUMA penalty) are
dwarfed by the shared-data migration cost traditional locks pay (8192 misses
× ~50ns each). Our prefetch optimization (Phase 5, `_mm_prefetch` for next
PQ root's data before executing current CS) further amortizes this cost.
**Must validate empirically** with PMU counters in Group 10 (perf stat).

**Prefetcher caveat (empirical finding, 2026-03-01):** Sequential access
patterns allow the hardware prefetcher to mask cross-core data migration
cost, making delegation appear to provide no throughput advantage over
traditional locks (FC/ShflLock ≈ 1.0x with sequential access). **Random
access patterns are essential** to expose the true migration penalty. With
random access on a 512 KiB array (exceeds L1d = 48 KiB on Sapphire
Rapids), delegation achieves ~2x throughput over ShflLock/MCS at 32
threads. This is consistent with real-world access patterns: hash tables,
B-trees, and log buffers exhibit pointer-chasing or key-hashed access,
not sequential scans. The counter-array benchmark now supports
`--random-access` and `--array-size` flags to control this.

**Implication:** Delegation is not just *a place* to add fairness — it is
*the right place*, because it uniquely decouples scheduling decisions from
data locality costs. Fair delegation should approach the throughput of
unfair delegation, while traditional fair locks necessarily sacrifice
throughput relative to unfair ones. This is a central claim we should
validate experimentally: plot throughput of FC vs FC-PQ vs MCS vs CFL,
showing that the FC→FC-PQ gap is much smaller than the MCS→CFL gap.

### 4.5 Summary Position

**Our position:** We are the first to bring scheduler-cooperative fairness
*into* the delegation pattern itself. Unlike CFL/ShflLock (which schedule
off-path over a distributed queue with O(N) remote reads), we schedule
on-path inside the combiner over L1-hot data structures. Unlike U-SCL
(which retrofits fairness onto traditional MCS with lock-slice banning),
we exploit delegation's centralized execution model to achieve
usage-fairness without shared-data migration. Unlike TCLocks (which solve
API transparency), we solve fairness — and the two are complementary.
Syncord's finding that reordering alone cannot strictly enforce fairness
validates our banning approach and motivates our priority-based combining
as a work-conserving alternative.

### 4.6 Optimizations Inspired by Related Work

Several ideas from the related work could improve our implementation:

1. **CFL's vLHT initialization for newcomers.** CFL initializes a new
   thread's vLHT to the current average vLHT, preventing starvation of
   existing threads by newcomers with zero usage. Our FC-PQ/FC-SL currently
   initialize usage to 0, meaning a newly arriving thread will be served
   before all existing threads regardless of contention. We should
   initialize new threads' usage to the current average (or median) to
   prevent this priority inversion.

2. **CFL's NUMA grace period.** CFL's `min_sid` policy temporarily groups
   same-socket threads even if it violates strict fairness ordering. We
   could add a similar NUMA-aware tie-breaking in FC-PQ: among threads
   with similar usage (within an epsilon), prefer the one on the same NUMA
   node as the combiner. This preserves fairness while reducing cross-node
   cache traffic for the delegate function.

3. **Syncord's insight: hybrid admission control.** FC-PQ is
   work-conserving but doesn't prevent a bursty thread from consuming
   disproportionate lock time before the priority queue catches up. We
   could add a lightweight "soft ban" to FC-PQ: if a thread's usage
   exceeds 2x the average, delay its insertion into the priority queue
   by one combining pass. This combines the strict enforcement of banning
   with the work-conservation of priority scheduling.

4. **TCLocks' waiter data prefetching.** TCLocks prefetch the waiter's
   stack data during the context switch. In FC-PQ, the combiner knows
   which node it will serve next (the heap root) and could prefetch that
   node's request data while executing the current CS. This hides the
   memory latency of loading the next waiter's input.

5. **ShflLock's batch counter for starvation bounds.** ShflLock uses a
   `MAX_SHUFFLES` counter to bound how many times a thread can be
   pushed back. We could add a similar starvation counter to FC-PQ:
   after being skipped K times, a thread's usage is clamped to the
   current minimum, guaranteeing bounded wait.

---

## 5. System Design

### 5.1 Lock Abstraction (DLock2 Trait)

```
trait DLock2<T, I, F>:
    fn lock(&self, data: I) -> I
```

The delegation API takes input data `I`, executes delegate `F` on shared state
`T`, and returns the result. This is fundamentally different from mutex
acquire/release — the critical section is a value, not a code region.

### 5.2 Banning Strategy

**Invariant:** A thread's lock-holding time over any window should be
proportional to 1/N, where N is the number of active threads.

**Mechanism (FC-Ban):** After executing a thread's critical section, the
combiner computes:

```
penalty = cs_time × num_waiting_threads
banned_until = current_tsc + penalty
```

On subsequent combining passes, banned threads are skipped. The combiner-side
check adds overhead (traversing banned nodes) but requires no waiter
modification.

**Mechanism (CC-Ban):** Waiters self-exclude from the job queue while banned.
The combiner records the critical section length and communicates it back. This
avoids the combiner traversing empty nodes but requires waiter-side logic.

### 5.3 Priority-Based Combining

**Invariant:** Threads with less cumulative lock usage are served first,
achieving CFS-like proportional fairness.

**Mechanism (FC-PQ):** Threads announce via a lock-free ring buffer. The
combiner drains announcements into a sequential priority queue (BinaryHeap or
BTreeSet) ordered by cumulative usage. The combiner pops the lowest-usage
thread, executes its CS, and updates usage: `usage += cs_time`.

**Mechanism (FC-SL):** Threads push into a concurrent skip list ordered by
cumulative usage. The combiner pops from the front (lowest usage). This
eliminates the two-phase announce-then-insert overhead of FC-PQ.

### 5.4 Combiner Selection Optimization

**Tail-as-combiner:** In FC, the last node in the traversal is executed last.
If this thread becomes the combiner, its response time (= sum of all CS in the
pass) is no worse than it would be as a waiter. Head-as-combiner, by contrast,
inflates response time from own-CS to sum-of-all-CS.

---

## 6. Evaluation Plan

### 6.1 Machines

| Machine | Cores | Threads | Architecture | NUMA |
|---------|-------|---------|-------------|------|
| Intel Xeon Gold 6438M (saturn) | 64 | 128 | Sapphire Rapids | 2 nodes |
| AMD EPYC (TBD) | TBD | TBD | Zen 3/4 | TBD |
| Cloudlab c6525-100g (fallback) | 32 | 64 | AMD EPYC 7543 | 2 nodes |

Running on at least two machines (one Intel, one AMD) is necessary for
a credible evaluation. TCLocks (OSDI'23) used an 8-socket, 224-core Intel
machine; our 2-socket Saturn is competitive for the common 2-socket NUMA
scenario. Need AMD for cross-vendor validation (different L3 topology,
Infinity Fabric interconnect).

### 6.2 Lock Variants Under Test

**Traditional (unfair):**
- Mutex (Rust std — spins 100 iters then futex)
- SpinLock (TTAS with backoff)
- MCS Lock (queue-based, acquisition-fair)
- ShflLock (MCS + NUMA-aware queue shuffling)
- PthreadMutex (glibc NPTL, for reference)

**Traditional (fair):**
- CFL (MCS + off-path vLHT queue reordering)
- Ticket Lock (FIFO, O(N) invalidations per handoff)
- CLH Lock (FIFO, spins on predecessor)

**Delegation (unfair):**
- FC (Flat Combining)
- CCSynch
- DSMSynch

**Delegation (our fair variants):**
- FC-Ban, CC-Ban (banning)
- FC-PQ (BinaryHeap), FC-PQ (BTree) (priority-based)

**Scheduler-cooperative:**
- U-SCL (lock-slice based, traditional MCS underneath)

### 6.3 Micro-Benchmarks

**Methodology:** Each experiment runs 3 trials, 15s per trial for throughput
(30s for final paper runs, following TCLocks' 30-second convention), 5s for
latency experiments. Threads are pinned to cores. 2s warmup before measurement.

#### Shared Counter (Proportional CS)
- CS ratio sweep: 1:1, 1:3, 1:10, 1:30, 1:100 (heterogeneous CS)
- Non-CS sweep: 0, 10, 100, 1K, 10K, 100K (contention levels)
- Thread counts: 4, 8, 16, 32, 64, 128
- Metrics: total throughput, per-thread throughput, Jain's fairness index,
  response time CDF (combiner vs. waiter split), p50/p95/p99/p99.9

#### Counter Array (Data Footprint)
- Shared `Vec<u64>` with configurable size via `--array-size` (default 4096
  elements = 32 KiB). Each CS touches `cs_loop` elements.
- **Random access mode** (`--random-access`): xorshift PRNG index generation
  defeats hardware prefetching, exposing the true cache migration cost.
  Sequential access lets the prefetcher hide migration (FC/ShflLock ≈ 1.0x);
  random access reveals the real gap (FC/ShflLock ≈ 2x at 512 KiB).
- Array size sweep: 4096, 8192, 16384, 32768, 65536, 131072 elements
  (32 KiB – 1 MiB). Key boundary: >6144 elements (48 KiB) exceeds L1d
  on Sapphire Rapids.
- Both sequential and random access for each size (sequential = null result
  showing prefetcher masks migration; random = key result).
- Isolates data migration cost from CS duration
- Key result: delegation advantage grows with working set size and is
  only visible with random access patterns (realistic for hash tables,
  B-trees, log buffers)

#### Fetch-and-Multiply
- Ultra-short CS (single multiply)
- Non-CS: random 1–8 iterations
- Compares against lock-free CAS baseline
- Shows overhead floor of fairness mechanisms

#### Factor Analysis (inspired by TCLocks OSDI'23)
- Decompose FC-PQ's overhead relative to FC into additive components
- Sequence: FC (base) → FC + PQ scheduling → FC-PQ (with all optimizations)
- Also: FC-PQ without prefetch → FC-PQ with prefetch → FC-PQ with starvation bound
- 32 threads, CS=1000,3000, 15s. Compare throughput and JFI at each step.
- Analogous to TCLocks' Fig 5g (Base → +NUMA → +Prefetch → +WWJump)
- Key result: most of FC→FC-PQ overhead is the PQ ops (O(log N)), not fairness enforcement

### 6.4 End-to-End Applications (TO BE IMPLEMENTED)

TCLocks' evaluation uses real kernel applications (Psearchy, Metis) and
userspace workloads (LevelDB) beyond micro-benchmarks. We need at least
one application benchmark to match this evaluation standard. Our applications
are userspace (not kernel), but they demonstrate real-world patterns where
fairness matters.

**Important distinction from TCLocks:** TCLocks can transparently replace
`spinlock_t` in the kernel — zero code changes. Our delegation API
requires restructuring CS into delegate functions. This means we cannot
simply drop into arbitrary applications. Our benchmarks must be designed
as delegation-compatible workloads that represent real-world access
patterns (heterogeneous CS, reader-writer asymmetry, scan vs point-lookup).
Reviewers may call these "complex micro-benchmarks" rather than
"real applications" — we should be honest about this and argue that the
patterns (not the specific applications) are what matter.

#### A. Concurrent Hash Map (Priority: Very High)
- Lock-protected hash map, single global delegation lock
- Operations: get (short CS ~100ns), put (medium ~500ns), scan (long ~5-50us)
- Thread mix: N-2 lookup threads + 2 scan threads
- Pre-populate 10K entries, Zipfian key distribution
- **Hypothesis:** Fair delegation locks prevent scan threads from starving
  lookup threads; lookup p99 stays bounded under FC-PQ

#### B. Producer-Consumer Log Buffer (Priority: High)
- `VecDeque<LogEntry>`, single global delegation lock
- N-1 producers (short CS: append one entry) + 1 consumer (long CS: drain batch)
- Variable drain batch size: K=100, 250, 500
- **Hypothesis:** FC-PQ throttles consumer's long CS, maintaining producer throughput

#### C. (Optional) LevelDB Benchmark
- Wrap LevelDB's global DB lock with delegation API, similar to TCLocks' Fig 6
- `readrandom` workload with 1M KV pairs, mixed read/write
- Would provide direct comparison point with TCLocks' evaluation
- Requires C++ FFI integration — moderate effort but high credibility value

### 6.4.1 Application Integration Paths (Research Notes)

Three approaches exist for integrating delegation locks into real applications.
Analysis based on TCLocks source code review (rs3lab/TCLocks on GitHub):

**Approach 1: Explicit delegate API (current).** Our DLock2 trait requires
restructuring CS into `lock(input) -> output` calls. Code change factor
~8-10x vs std::sync::Mutex. Suitable for purpose-built benchmarks (hash map,
log buffer) but not for retrofitting existing applications.

**Approach 2: LD_PRELOAD transparent delegation (TCLocks-style).** TCLocks
already has a working userspace port (`src/userspace/litl/`) that intercepts
`pthread_mutex_*` via the LiTL framework. Core mechanism: lightweight
coroutine-style stack switch via 14 instructions of x86-64 inline assembly
(`komb_context_switch`). Each thread gets an 8KB thread-local shadow stack.
The combiner swaps RSP to the waiter's saved stack, the waiter's code resumes
normally through unlock, then control returns to the combiner. ~47ns overhead
per waiter. Works in userspace because no kernel privileges needed for RSP
manipulation. Key concern: signal safety (signals on wrong thread's stack).

**Approach 3: QD Lock automatic transformation (Uppsala).** Klaftenegger,
Sagonas & Winblad (Euro-Par 2014, IEEE TPDS 2017) built a tool that
automatically transforms C `pthread_mutex_lock/unlock` pairs into QD Lock
delegation calls. For Rust, a proc-macro could do the same for simple
`Mutex::lock()` / `guard` / `drop` patterns.

**For the paper:** Focus on Approach 1 (explicit API) for hash map and log
buffer. Mention Approach 2 in Discussion as future work: "Our fairness
mechanisms (priority-based combining) are orthogonal to the transparency
mechanism (stack-switching). Combining TCLocks' transparent delegation with
FC-PQ's priority scheduling would yield fair transparent delegation —
benefiting unmodified applications."

**Key insight from TCLocks source:** Their NUMA-aware `get_next_node()`
reorders the waiter queue for locality without measuring or bounding
fairness. This proves reordering is free in delegation (shared data stays
in combiner's L1 regardless of order), directly supporting our thesis that
FC-PQ's priority reordering for fairness is also essentially free.

### 6.5 Analysis & Metrics

**Throughput:** Total operations/second across all threads.

**Fairness (primary, following U-SCL):** Per-thread lock-holding time
normalized by expected fair share. For equal-weight threads, the expected
fair share is 1/N of total lock-holding time. Plot as per-thread bar charts
where perfectly fair = all bars equal height. This matches the evaluation
style of Patel et al. (EuroSys'20) and directly shows which threads are
over/under-served.

**Fairness (summary):** Jain's Fairness Index (Jain, Chiu & Hawe,
DEC-TR-301, 1984) as a single-number summary for tables:
```
JFI = (Σ xᵢ)² / (n × Σ xᵢ²)
```
where xᵢ = lock-holding time of thread i, normalized by wall-clock time.
JFI = 1.0 is perfectly fair; JFI = 1/n is maximally unfair. Useful for
comparing across many configurations at a glance, but less informative
than the per-thread breakdown.

**Tail Latency:** p50, p95, p99, p99.9, p99.99 response times, both overall
and split by combiner vs. waiter role. TCLocks reports p99.99 — we should
match this. Our combiner/waiter split is a unique contribution they lack.

**Overhead:** perf stat profiles (cache misses, L1/L2/LLC misses, dTLB
misses, CPU migrations) for each lock variant.

**Fairness-Throughput Tradeoff:** Scatter plot of (throughput, JFI) for each
lock variant × thread count, showing Pareto frontier. This is the paper's
**central result figure**: FC-PQ should appear in the upper-right (high
throughput + high JFI) while CFL appears upper-left (high JFI, low
throughput) and FC appears lower-right (high throughput, low JFI).

**Factor Analysis (new, inspired by TCLocks):** Decompose FC-PQ's
throughput overhead vs FC into additive contributions from each mechanism.
Plot stacked bar or waterfall chart showing: base FC throughput → cost of
PQ scheduling → cost of starvation bound → net FC-PQ throughput.
Analogous to TCLocks' Fig 5g factor decomposition.

---

## 7. Paper Outline

1. **Introduction** (1.5 pages)
   - Delegation locks recap; throughput advantage
   - Usage-unfairness example (CCSynch with 10ns vs 30ns CS)
   - Combiner penalty observation
   - Our approach: combiner-as-scheduler

2. **Background & Motivation** (1.5 pages)
   - Flat Combining, CCSynch, RCL overview
   - Scheduler subversion (cite U-SCL)
   - Quantitative motivation: unfairness under heterogeneous CS

3. **Design** (3 pages)
   - Usage-fairness definition
   - Banning strategy (FC-Ban, CC-Ban)
   - Priority-based combining (FC-PQ, FC-SL)
   - Combiner selection optimization
   - Implementation in Rust (DLock2 framework)

4. **Evaluation** (3.5 pages)
   - Experimental setup (machines, pinning, methodology)
   - **Fairness-throughput Pareto scatter** (the central result)
   - **Factor analysis** (FC → FC-PQ overhead decomposition)
   - Micro-benchmark throughput (CS ratio sweep, thread scaling)
   - Fairness analysis (per-thread hold-time, Jain's index)
   - Response time distributions (combiner vs waiter split CDFs)
   - Combiner penalty characterization (penalty ratio vs thread count)
   - Delegation vs ShflLock head-to-head (fairness, L1 locality, tail latency)
   - Data footprint scaling (array size sweep, sequential vs random access —
     shows prefetcher masks migration cost; random access reveals 2x advantage)
   - End-to-end applications (hash map, log buffer)
   - Cache miss validation (perf stat: L1/LLC misses)

5. **Discussion** (0.5 pages)
   - Transparent fair delegation (TCLocks + FC-PQ combination — future work)
   - NUMA-aware fair combining (H-Synch extension)
   - Async/coroutine combiner vision (future work)

6. **Related Work** (1 page)

7. **Conclusion** (0.5 page)

---

## 8. Target Venues

| Venue | Fit | Deadline (approx.) | Notes |
|-------|-----|-------------------|-------|
| **PPoPP** | Best | Aug 2026 | Core synchronization audience |
| **EuroSys** | Strong | Oct 2026 | Need strong application story |
| **USENIX ATC** | Good | Jan 2027 | Practical systems focus |
| **ASPLOS** | Possible | Varies | If HW/cache angle is strong |

**Recommendation:** Target PPoPP 2027 (deadline ~Aug 2026) as primary.
Fallback to EuroSys 2027 (deadline ~Oct 2026) if application evaluation
needs more time.

---

## 9. Open Questions & Risks

1. **Is the throughput-fairness tradeoff significant enough?** If fair variants
   only lose 5% throughput while gaining near-perfect fairness, the story is
   compelling. If the cost is 30%+, we need to argue harder about when fairness
   matters. The factor analysis (Group 12) directly measures this.
   **Update (2026-03-01):** At large CS (array-size=65536, CS=4096), FC-PQ
   is within 2% of FC throughput — fairness is essentially free. The cost
   is only significant at ultra-short CS (single counter: FC-PQ = 0.74× FC).

2. **Do real applications actually suffer from delegation unfairness?** The
   end-to-end benchmarks must demonstrate measurable impact — not just
   theoretical concern. Reviewers may argue our hash map / log buffer are
   "complex micro-benchmarks" rather than real applications. We should be
   honest about this limitation and argue the access patterns are representative.
   A LevelDB integration (Section 6.4C) would significantly strengthen credibility.

3. **How does NUMA affect the fairness mechanisms?** Banning a remote-NUMA
   thread has different cost implications than banning a local one. Fair
   reordering may cause non-linear NUMA access to waiter inputs. Must validate
   with PMU counters (Group 10) that input-fetch NUMA penalty is small relative
   to shared-data migration savings.

4. **Is the async/coroutine combiner idea implementable?** Even a prototype
   would massively strengthen the paper, but it's high-risk. A combiner that
   yields back to an async runtime during CS execution would elevate this
   from "better lock" to "next-gen concurrency primitive." Defer to future work
   unless core evaluation completes early.

5. **Reviewer pushback on Rust-only implementation?** Systems conferences
   sometimes prefer C/C++ for lock papers. Mitigated by our C baseline
   implementations (FC-C, CC-C, ShflLock-C) connected via FFI. These validate
   that Rust implementations match C performance. Argue Rust's zero-cost
   abstractions make the comparison fair.

6. **Combiner pass time-bounding.** Should the combiner yield mid-pass if it
   exceeds a time quota? TCLocks uses `WAITERS_TO_COMBINE = 1024` as a count
   bound. A time-based bound would cap combiner latency but risks
   work-conservation violations. This is the batch-size sensitivity analysis
   (TCLocks Fig 5i). Consider as future optimization if combiner penalty
   data (Phase 2) shows it's a problem in practice.

7. **Hardware prefetching masks delegation's throughput advantage in
   sequential benchmarks.** (Discovered 2026-03-01.) With sequential array
   access, FC/ShflLock ≈ 1.0x regardless of data footprint — the prefetcher
   compensates for cross-core migration. Random access is required to expose
   the real gap (FC/ShflLock ≈ 2x at 512 KiB, 32 threads). This means:
   - Counter-array experiments **must** use `--random-access` to show
     delegation's locality advantage.
   - The paper must present both sequential (null result) and random (key
     result) to be honest about when delegation helps.
   - Real-world workloads (hash tables, B-trees) naturally have random/
     pointer-chasing access, so the random-access result is the
     representative one. But reviewers may challenge this — be ready to
     argue that sequential counter is the unrealistic case, not random.
   - The hash map application benchmark (Phase 6) will exhibit random access
     by construction (key-hashed lookups), providing a natural validation.

---

## 10. Timeline

| Phase | Duration | Deliverable |
|-------|----------|-------------|
| Formalize metrics & cleanup | 2 weeks | Jain's index integration, response time CDF tooling |
| Combiner penalty study | 2 weeks | Characterization across thread counts, tail-as-combiner eval |
| Second machine experiments | 2 weeks | NUMA machine results |
| End-to-end app: concurrent hash map | 3 weeks | YCSB-style benchmark with fairness measurement |
| End-to-end app: priority queue server | 2 weeks | Mixed-operation workload |
| Additional baselines (MCS, CLH, ticket) | 1 week | Complete comparison set |
| Overhead breakdown | 1 week | perf stat analysis for all variants |
| Writing: design + evaluation | 3 weeks | Core paper sections |
| Writing: intro + related work + polish | 2 weeks | Complete draft |
| Internal review + revision | 2 weeks | Camera-ready quality |
| **Total** | ~16 weeks | |
