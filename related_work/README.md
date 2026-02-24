# Related Work

Papers collected for the usage-fair delegation locks project, organized by
category. Each entry has a one-paragraph summary and notes on relevance.

---

## Delegation / Combining Locks

### Flat Combining (Hendler, Incze, Shavit & Tuttle, SPAA'10)
Threads publish requests to a shared list; one thread becomes the *combiner*,
executes all pending operations on the shared data structure, and writes back
results. Eliminates data migration across cores — the shared structure stays
in the combiner's L1 cache. Throughput scales well under contention.
**Gap:** No fairness mechanism. Threads with longer CS consume more lock time.
The combiner bears disproportionate latency (sum of all CS in the pass).

### CCSynch / DSMSynch (Fatourou & Kallimanis, PPoPP'12)
Queue-based delegation. CCSynch: waiters enqueue requests, the combiner
processes them in FIFO order. DSMSynch: distributed variant with double-buffer
swap. Both achieve high throughput with FIFO acquisition ordering.
**Gap:** FIFO acquisition order does *not* imply usage-fairness. A thread with
3x longer CS gets 3x the lock-holding time. No mechanism to equalize usage.

### RCL (Lozi, David, Thomas, Lawall & Muller, ATC'12)
Remote Core Locking: a dedicated server core executes all critical sections.
Eliminates combining overhead but wastes a core. Excellent for short CS.
**Gap:** No fairness mechanism. The server processes requests in arrival order.

### FFWD (Roghanchi, Eriksson & Basu, PPoPP'17)
Fast delegation via a dedicated core with single-writer mailboxes. Removes
combiner election overhead entirely. Highest throughput among delegation locks.
**Gap:** No fairness mechanism. Dedicated core is wasteful for low contention.

### TCLocks (Gupta, Kim, Rao, Min & Bueso, OSDI'23)
**File:** `TCLocks-OSDI23.pdf`

Transparent delegation — wraps existing mutex acquire/release API by capturing
RSP/RIP at lock acquisition and replaying the critical section on the
combiner's stack via lightweight context switching (~47ns overhead). Uses
ephemeral stacks to prevent concurrent stack access. Handles nested locking
and out-of-order unlocking. Built on TAS+MCS with DSMSynch-style combining.
NUMA-aware via CNA-inspired queue splitting. Batches up to 1024 waiters.
**Key insight:** Delegation's throughput advantage can be obtained without
API changes — the lock/unlock interface is preserved by capturing the
program counter and stack pointer at acquisition time.
**Limitations noted by authors:**
- Resource accounting: combiner thread gets CPU credit for waiter's CS work,
  confusing the OS scheduler. Directly related to our fairness problem.
- `current` macro: kernel thread identity is wrong during delegated execution.
- Overhead at 2-4 cores: context switching cost dominates at low contention.
**Gap:** Solves the API transparency problem, not the fairness problem.
Complementary to our work: fair delegation + transparent API is ideal.
**Relevance to our work:** TCLocks' resource accounting limitation is the
flip side of our scheduler subversion problem — the OS doesn't know that
the combiner is doing work on behalf of others. A fair TCLock would need
to charge CS time back to the correct thread.

---

## Scheduler-Cooperative / Fair Locks

### U-SCL (Patel, Wires, Birrell, 2020)
**File:** (not in folder — original code available on GitHub)

Scheduler-Cooperative Locks with usage-time accounting. Each thread gets a
*lock-slice* (analogous to CPU time-slice). After exhausting its slice, the
thread is banned for a duration proportional to `cs_time * (total_weight /
thread_weight)`, using Linux CFS's `prio_to_weight` table. The ban is
implemented by sleeping (nanosleep + spin). Built on MCS lock.
**Key mechanism:** `banned_until += cs * (total_weight / weight)` — same
banning formula we adapted for FC-Ban.
**Limitation:** Non-work-conserving — banned threads waste CPU or sleep, even
if no one else wants the lock. Lock-slice approach degrades when non-CS is
long (the slice expires during non-CS).
**Fairness metric:** Per-thread lock-holding time relative to scheduling
weight. No single-number fairness index.

### CFL — Completely Fair Locking (Manglik, Kim, 2024)
**File:** `CFL-PPoPP24.pdf`

Queue-based lock (CFL-MCS, CFL-ShflLock) with an embedded *Lock Scheduler*
(LS) thread. The LS sits in the wait queue and uses its idle time to reorder
waiters by *virtual Lock Hold Time* (vLHT) — analogous to vruntime in CFS.
Threads that have held the lock longer get pushed back in the queue.
**Key properties:**
- Work-conserving: no thread is banned; reordering only happens among waiters
- LS scheduling is OFF the critical path (the LS is a waiter, not the holder)
- NUMA-aware: `min_sid` policy keeps socket-local threads grouped
- Cgroup integration: hierarchical weight support
**Limitations:**
- LS traverses N foreign qnodes per pass → O(N) cross-core cache misses
- LS must CAS on each qnode to reorder → contention with new arrivals
- Works only with queue-based locks (MCS/CLH family), not delegation
- Each lock instance needs a dedicated LS thread
**Comparison to our work:**
- CFL's scheduling is off-path but cache-unfriendly (N remote reads)
- Our combiner scheduling is on-path but cache-friendly (L1-hot heap/skiplist)
- CFL is work-conserving; FC-Ban is not, but FC-PQ/FC-SL are
- CFL supports cgroups and NUMA; we do not yet
- CFL applies to traditional locks; we apply to delegation locks

### ShflLock (Kashyap, Prasad, Rajwar, 2019)
**File:** `ShflLock-SOSP19.pdf`

Scalable locking via *shuffling*: waiter threads reorder the MCS queue OFF the
critical path based on pluggable policies. Primary policy: NUMA-awareness
(group same-socket waiters together to reduce cache-line migration). Secondary:
parking/wakeup (move threads that will be parked to the back). Built on
TAS+MCS combination with 12-byte per-lock footprint.
**Key insight shared with CFL:** use idle waiter time for off-path scheduling.
CFL extends this idea from NUMA-awareness to usage-fairness.
**Not about fairness itself** — the shuffling policies optimize performance and
NUMA locality, not usage-time equalization.
**Fairness metric used:** Dice et al.'s metric — sort per-thread ops, divide
upper-half sum by total. 0.5 = fair, 1.0 = unfair.
**Relevance:** Establishes the pattern of waiters doing useful off-path work.
Our delegation approach uses the combiner instead (on-path but L1-hot).
This is a fundamental architectural contrast.

### Syncord (Park, Lim, Min, 2022)
**File:** `Syncord-OSDI22.pdf`

Application-Informed Kernel Synchronization Primitives. A framework for
dynamically patching kernel lock policies at runtime using eBPF (for policy
logic) and Livepatch (for lock implementation swapping). Exposes 10 API hooks:
`should_reorder`, `skip_reorder`, `backoff`, `lock_to_acquire`, etc.
Demonstrates three policies: Reader-Writer (reorder readers together), Bully
(ban long-CS threads), and NUMA (group same-socket).
**Key empirical finding:** Reordering alone does not strictly enforce fairness.
SCL+ReorderBully had 1659 policy violations vs. SCL+BackoffBully with 538.
The backoff formula: `wait = lock_hold_time[curr] × num_threads − total_lock_hold_time`.
**Relevance to our work:**
- Validates that admission control (banning/backoff) is necessary for strict
  fairness enforcement. Pure reordering (as in CFL) improves fairness
  probabilistically but cannot guarantee it.
- Their framework demonstrates the design space; we provide concrete, well-
  characterized algorithms at specific points in that space.
- The backoff formula is closely related to our banning formula
  (`penalty = cs_time × N_waiting`).

---

## Classic Lock Algorithms (Baselines)

### MCS Lock (Mellor-Crummey & Scott, 1991)
Queue-based spin lock. Each thread spins on its own local variable, achieving
O(1) cache coherence traffic. FIFO acquisition order (acquisition-fair).
**Relevance:** Canonical acquisition-fair baseline. Shows that FIFO ordering
does not prevent usage-unfairness under heterogeneous CS.

### CLH Lock (Craig, 1993; Landin & Hagersten, 1994)
Queue-based spin lock similar to MCS but spins on predecessor's node.
Cache-friendlier on some architectures (implicit queue via pointers).
**Relevance:** Another acquisition-fair baseline.

### Ticket Lock
Simple FIFO spin lock using fetch-and-add for ticket dispensing and a shared
`now_serving` counter. O(N) coherence traffic per handoff but strictly fair
in acquisition order.
**Relevance:** Simplest acquisition-fair lock. Already implemented in C in
this repo (`c/ticket/ticket.c`).

---

## Fairness Metrics

### Jain's Fairness Index (Jain, Chiu & Hawe, DEC-TR-301, 1984)
Originally for network bandwidth allocation. Formula:
```
JFI = (sum xi)^2 / (n * sum xi^2)
```
Ranges from 1/n (maximally unfair) to 1.0 (perfectly fair). We use this as
a single-number summary metric in tables. CFL also uses this metric.

### Dice et al. Fairness Factor
Sort per-thread operation counts, divide sum of upper half by total sum.
0.5 = perfectly fair, 1.0 = maximally unfair (one thread does everything).
Used by ShflLock.

---

## Files in this folder

| File | Paper | Venue |
|------|-------|-------|
| `CFL-PPoPP24.pdf` | Completely Fair Locking | PPoPP 2024 |
| `ShflLock-SOSP19.pdf` | Scalable and Practical Locking with Shuffling | SOSP 2019 |
| `Syncord-OSDI22.pdf` | Application-Informed Kernel Synchronization Primitives | OSDI 2022 |
| `TCLocks-OSDI23.pdf` | Ship your Critical Section, Not Your Data | OSDI 2023 |
