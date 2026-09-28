# Algorithm Improvement Plan: Beyond FC-PQ

Date: 2026-03-23

Analysis update: 2026-09-23 — see [LogP analysis proposal](#logp-analysis-proposal).
This update analyzes the implemented locks; it does not approve or implement
FC-EW or the other algorithm changes below. Earlier unconditional fairness
claims in this plan must be read with the cohort and accounting assumptions
specified in the analysis.

## Goal

Improve the current fairness algorithm without expanding the project into too
many disconnected variants. The main objective is to keep the strongest part
of the existing story:

- delegation preserves shared-data locality
- fairness should be cheaper to add in delegation than in queue locks

But the algorithm should do more than exact min-usage ordering. It should:

1. retain strong usage-fairness guarantees
2. reduce combiner-side overhead when exact ordering is unnecessary
3. directly address combiner latency asymmetry
4. turn fairness mechanisms on only when they are needed
5. create a path toward handling long operations better than any
   non-preemptive lock can

This plan proposes a new primary design direction:

- `FC-EW`: Eligibility-Window Fair Combining

and two follow-on extensions:

- combiner-budgeted fair combining
- resumable/sliced delegated operations

## High-Level Recommendation

Do not spend more time on additional priority-queue variants.

- Keep `FC-PQ` as the correctness and theory anchor.
- Introduce `FC-EW` as the practical algorithmic improvement.
- Treat `FC-Ban` as a strict-admission reference point, not the long-term
  mainline design.
- De-emphasize `FC-SL`; it is unlikely to matter because the combiner side is
  sequential.

## Why FC-PQ Is Not the End State

`FC-PQ` is strong because exact min-usage service gives a clean fairness bound.
But it also hard-codes one very specific policy:

- always serve the exact minimum-usage waiter next

That is useful for analysis, but it is not necessarily the best system design.
It has three practical limitations:

1. It optimizes only service fairness, not combiner burden fairness.
2. It pays ordering cost even when many waiters are "fair enough."
3. It cannot improve the fundamental `C_max` limit for long operations because
   delegated work is still non-preemptive.

So the next algorithmic step should not be "better heap engineering." It
should be a scheduler that preserves fairness bounds while relaxing exact
ordering and incorporating combiner-specific control.

## Proposed Primary Algorithm: FC-EW

### Core Idea

Replace exact global min-ordering with an eligibility window.

Maintain:

- `usage_i`: cumulative lock-holding time charged to thread `i`
- `u_min`: minimum usage among currently eligible or waiting threads
- `delta`: fairness slack parameter

Define a waiter as eligible when:

```text
usage_i <= u_min + delta
```

The combiner only needs to choose among eligible waiters, not among the full
set in exact total order.

### Why This Helps

This creates a continuum:

- `delta = 0`: equivalent to exact min-usage scheduling
- small `delta`: strong fairness with modest scheduling freedom
- large `delta`: behavior approaches plain FC

This is better than the current split between "PQ reordering" and "banning"
because one mechanism can express both:

- fairness control
- mild locality preference
- low-overhead operation under near-homogeneous workloads

### Data Structures

Use two structures:

1. `eligible_queue`
   - FIFO or ring buffer
   - contains threads whose usage is within the window

2. `deferred_set`
   - min-ordered by `usage_i`
   - only used to promote threads into the eligible window when `u_min`
     advances

The key point is that the combiner does not need to heap-pop every operation.
It can often run from `eligible_queue` for many services before revisiting the
ordered structure.

### Secondary Scheduling Rule

Among eligible waiters, choose with a simple secondary policy. Start with FIFO.
Possible later policies:

- FIFO among eligible waiters
- same-socket preference among eligible waiters
- shortest-predicted-CS among eligible waiters
- highest response-time debt among eligible waiters

The first implementation should use FIFO to minimize complexity.

### Fairness Target

The intended empirical invariant is:

```text
max_i,j |usage_i - usage_j| <= delta + C_max
```

Expected reasoning:

- a thread can only run ahead by at most the window slack before it is no
  longer eligible
- one service can add at most `C_max`

This is slightly weaker than exact PQ, but still strong and much easier to
trade against throughput.

### Pseudocode Sketch

```text
combine():
  refill_eligible()
  while budget_allows() and eligible_queue not empty:
    t = pick_from_eligible_queue()
    execute(t.request)
    charge usage_t += measured_cs_time
    update u_min if needed
    refill_eligible()

refill_eligible():
  while deferred_set.min_usage <= u_min + delta:
    move deferred_set.min -> eligible_queue
```

This is the base algorithm. The remaining improvements control who combines,
when a combining pass stops, and how long requests are allowed to monopolize
the combiner.

## Improvement 1: Combiner Debt and Time Budget

### Problem

The current fairness metric only accounts for who gets served. It does not
account for who pays the combiner overhead.

That leaves Problem B unresolved:

- a thread can be fair in service share
- yet still suffer disproportionate latency because it often becomes combiner

### Proposed State

Add:

- `service_usage_i`: cumulative delegated CS time received by thread `i`
- `combiner_work_i`: cumulative time thread `i` spent as combiner
- `pass_budget_tsc`: max combiner pass duration

### Scheduling Objective

Separate two debts:

1. service debt
   - who has consumed more lock time than peers

2. combiner debt
   - who has paid more coordination cost than peers

The combiner loop should:

- stop after a time budget, not only an item-count budget
- hand off combiner role preferentially to threads with low recent combiner
  burden or compensate threads with high combiner burden

### Concrete Plan

Phase A:

- add pass time budget
- terminate combine loop when elapsed TSC exceeds budget

Phase B:

- record combiner time per thread
- expose combiner-time skew as a first-class metric

Phase C:

- implement debt-aware combiner handoff:
  - candidate set: recently active waiting threads
  - pick next combiner with lowest `combiner_work_i`
  - break ties with service fairness state

### Expected Benefit

- caps worst combiner-side response inflation
- turns Problem B into a controlled policy variable
- avoids having combiner fairness remain only a measurement, not an algorithm

## Improvement 2: Adaptive Fairness Activation

### Problem

The repo's current framing already admits that fairness overhead is only worth
paying when:

- contention is high
- CS lengths are heterogeneous
- fairness imbalance is actually emerging

Under homogeneous or low-contention workloads, exact fair scheduling can be
pure overhead.

### Proposed Mechanism

Maintain cheap rolling estimates:

- CS-time variance
- current usage spread
- queue depth / contention level
- combiner-pass occupancy

Modes:

1. `FC_FAST`
   - plain FC behavior

2. `FC_EW`
   - eligibility-window fairness enabled

3. `FC_STRICT`
   - delta shrinks toward zero, or admission control becomes active

### Switching Policy

Example thresholds:

- if queue depth is low and usage spread is small: stay in `FC_FAST`
- if usage spread exceeds fairness threshold: move to `FC_EW`
- if spread persists or tail latency worsens: tighten `delta` or enable
  stricter admission control
- if the system stabilizes: relax back toward `FC_FAST`

### Expected Benefit

- keeps FC-like overhead in the easy cases
- makes the fair scheduler appear only when the workload needs it
- strengthens the practical "fairness is cheap" story because cost becomes
  demand-driven instead of always-on

## Improvement 3: Weighted Fairness

### Problem

The current fairness story is per-thread equal-share fairness. That is easy to
understand, but weak if the claim is scheduler cooperation.

### Proposed Extension

Assign each requester a weight `w_i` and schedule on normalized usage:

```text
vruntime_i = usage_i / w_i
```

Then FC-EW eligibility becomes:

```text
vruntime_i <= v_min + delta
```

### Why It Matters

- aligns the algorithm more directly with CFS-style reasoning
- prevents "spawn more threads" from trivially gaming fairness
- allows future experiments by tenant, request class, or priority class

### Scope

This should be a lightweight extension of FC-EW, not a separate algorithm.

## Improvement 4: Resumable / Sliced Delegation

### Problem

No non-preemptive scheduler can beat the single-operation bound imposed by
`C_max`. If one delegated request is a long scan, batch drain, or bulk insert,
then one service can still monopolize the combiner.

This is the largest remaining algorithmic limitation.

### Proposed Direction

Allow a delegated operation to yield after a bounded work chunk and requeue
itself with continuation state.

New request model:

```text
enum DelegateStepResult {
  Complete(Output),
  Pending(ContinuationState),
}
```

Each long operation becomes a series of chunks:

- scan 64 entries, yield
- drain 128 log items, yield
- insert K items in batches, yield

### Why This Is a Real Algorithmic Upgrade

It changes the fairness limit from:

- whole-request `C_max`

to:

- chunk-size bound

This directly improves:

- service fairness
- tail latency
- combiner latency

### Costs

- requires API changes
- requires continuation/state-machine style benchmark implementations
- increases design complexity and may not fit the first paper

### Recommendation

Treat this as the strongest medium-term direction, but only after FC-EW and
combiner budgeting are stabilized.

## Unification Strategy

The long-term algorithm family should be reduced, not expanded.

Target structure:

1. `FC`
   - unfair throughput baseline

2. `FC-PQ`
   - exact-fairness baseline and theory anchor

3. `FC-EW`
   - practical mainline algorithm

4. `FC-EW-Weighted`
   - minor extension of `FC-EW`, not a distinct major variant

5. `FC-EW-Sliced`
   - advanced extension for long operations

`FC-Ban` should remain only if experiments show strict admission control does
something FC-EW cannot approximate. `FC-SL` should likely be retired.

## Detailed Execution Plan

### Stage 0: Define Success Criteria

Before implementation, freeze the targets.

Required outcomes:

1. `FC-EW` must preserve a bounded fairness gap.
2. `FC-EW` must reduce scheduler overhead versus `FC-PQ` in at least some
   short-CS or moderate-contention regimes.
3. combiner time skew must become a first-class optimization target.
4. adaptive mode switching must not regress to pathological unfairness.

Deliverable:

- one short design note defining invariants and metrics

### Stage 1: Formalize FC-EW

Specify:

- exact meaning of `delta`
- which set defines `u_min`
- tie-breaking policy among eligible waiters
- how newcomers are initialized
- whether starvation counters still exist under FC-EW

Questions to settle:

1. Is `u_min` computed over all waiters, all known threads, or only active
   threads?
2. Does a newly arriving thread enter `eligible_queue` immediately if
   initialized at the running average?
3. Is `delta` fixed, or scaled by observed CS mean / variance?

Deliverables:

- pseudocode
- fairness invariant statement
- complexity summary

### Stage 2: Build the Simplest FC-EW Prototype

Implement only:

- fixed `delta`
- FIFO among eligible waiters
- no adaptive switching
- no weights
- no slicing

Instrumentation:

- per-thread `service_usage`
- window promotions
- size of `eligible_queue`
- time spent in ordered-structure maintenance

Goal:

- establish whether FC-EW already captures most of the benefit

### Stage 3: Fairness Validation for FC-EW

Add direct metrics that match the algorithm, not only JFI:

- max pairwise usage gap
- gap-over-time trace
- fraction of operations where `usage_i > u_min + delta + C_max`
- window occupancy histogram

This stage is essential. Otherwise the algorithm change is only justified by
throughput anecdotes.

### Stage 4: Combiner Budgeting

Add:

- TSC-based combine budget
- combiner-pass elapsed time tracking
- combiner-time accounting per thread

Experiments:

- short CS with many threads
- heterogeneous CS with long-tail requests
- compare no-budget, count-budget, time-budget

Primary metrics:

- combiner p99/p99.9 latency
- waiter p99/p99.9 latency
- combiner-time Jain index or max skew

### Stage 5: Debt-Aware Combiner Handoff

After time budgeting works, add next-combiner selection based on combiner
debt.

Simple first policy:

- among threads currently waiting and eligible to become combiner, choose the
  one with minimum `combiner_work_i`

Fallback:

- if the bookkeeping or handoff path becomes too intrusive, keep time-budgeted
  passes and expose combiner debt only as a metric

This stage is optional for the first paper, but it is the cleanest algorithmic
answer to Problem B.

### Stage 6: Adaptive Fairness Activation

Start with one conservative controller.

Inputs:

- queue depth
- recent usage spread
- CS variance estimate

Outputs:

- mode: `FAST` or `EW`
- `delta`
- optional strictness bit

Constraints:

- hysteresis required to avoid oscillation
- every mode transition must be logged
- fairness metrics must be checked during transitions

This should remain simpler than a PID controller in the first pass.

### Stage 7: Weighted Fairness

Minimal change:

- add `weight`
- replace raw usage with normalized virtual usage

Evaluation:

- equal weights should reduce to the original behavior
- 2:1 or 4:1 weighted-share experiment should match target ratios

This is high leverage for the paper narrative and low implementation risk.

### Stage 8: Resumable Delegation Prototype

Only attempt after the base scheduler is stable.

Prototype on one benchmark:

- hash map range scan
or
- log buffer drain

Implementation approach:

- explicit continuation object
- chunk budget in work units or TSC
- request requeued with updated continuation state

Success criterion:

- same total work
- lower p99 for short requests
- lower max usage gap because the long request is fragmented

## Experimental Plan for the New Algorithm

### A. Core Scheduler Comparison

Compare:

- `FC`
- `FC-PQ`
- `FC-EW`
- `FC-Ban` if retained

Workloads:

- homogeneous short CS
- moderate heterogeneity
- extreme heterogeneity
- asymmetric non-CS

Measure:

- throughput
- JFI
- max pairwise usage gap
- ordered-structure overhead

### B. Combiner Fairness Study

Compare:

- `FC`
- `FC-PQ`
- `FC-EW`
- `FC-EW + time budget`
- `FC-EW + debt-aware handoff`

Measure:

- combiner-time fraction per thread
- combiner/waiter split latency
- max combiner skew

### C. Adaptive Mode Study

Workload phases:

1. homogeneous / low contention
2. heterogeneous / high contention
3. homogeneous again

Goal:

- show the lock enters fair mode only when imbalance appears

### D. Weighted Fairness Study

Workloads:

- 2-class weights
- 3-class weights

Goal:

- delivered service share tracks configured weights

### E. Sliced Operation Study

One real workload with long operations:

- scan-heavy hash map
or
- producer/consumer log buffer

Goal:

- demonstrate improvement that exact PQ cannot achieve under whole-operation
  non-preemptive execution

## Theoretical Work Items

### For FC-EW

Try to prove:

```text
max_i,j |usage_i - usage_j| <= delta + C_max
```

Need to define carefully:

- active set
- newcomer initialization
- whether dormant threads count

### For Time Budgeting

Try to derive:

- upper bound on single combiner-pass time
- tradeoff between service fairness and pass interruption

### For Sliced Delegation

Try to show:

- replacing request bound `C_max` with chunk bound `Q_max`

This is likely the cleanest route to a stronger second theorem.

## Risks

1. `FC-EW` may behave too similarly to `FC-PQ` to justify a new algorithm.
   Mitigation: instrument scheduler overhead directly.

2. Adaptive mode switching may oscillate.
   Mitigation: use hysteresis and conservative thresholds.

3. Combiner debt accounting may complicate handoff too much.
   Mitigation: keep time-budgeting even if debt-aware handoff is deferred.

4. Resumable delegation may be too invasive for the current API.
   Mitigation: prototype on one benchmark only.

5. Too many variants may dilute the paper again.
   Mitigation: keep `FC-PQ` and `FC-EW` as the only two main fair designs.

## Recommended Priority Order

1. Formalize `FC-EW`
2. Prototype fixed-window `FC-EW`
3. Add direct fairness-gap metrics
4. Add combiner time budget
5. Evaluate against `FC-PQ`
6. Add adaptive switching
7. Add weighted fairness
8. Prototype sliced delegation on one workload

## Paper Positioning If This Works

If the plan succeeds, the story improves:

- `FC-PQ` provides the clean exact-fairness baseline and theorem
- `FC-EW` provides the practical scheduler that keeps most fairness at lower
  cost
- time-budgeting directly addresses combiner unfairness
- sliced delegation shows how to beat the non-preemptive `C_max` limit

That is a stronger algorithmic agenda than "more lock variants." It turns the
project from a set of fairness tweaks into a scheduling framework for
delegation locks.

## LogP Analysis Proposal

### Status and Scope

Initial proposal on 2026-09-23; source baseline `56a02ab`, isolated branch
`research/logp-analysis`. The user subsequently authorized developing a
publication analysis section; the current scope is recorded in the
[Publication Analysis Plan](../../plan/2026-09-23/logp-publication-analysis.md).
The proposal below is retained as background. No production lock code is changed.

**Recommendation:** use a LogP-inspired communication model plus a separate
non-preemptive scheduling model. The useful result is a conditional crossover
criterion and a scoped fairness theorem, not a claim that fairness is free.
Analyze the implemented DLock2 FC, FC-PQ, FC-Ban, CC/CC-Ban, and handoff baselines
first. FC-EW, weights, slicing, and dedicated-server variants are extensions,
not assumptions about the current implementation.

### Model Boundary: What LogP Does and Does Not Supply

Classical LogP [1] describes small-message communication with latency bound
`L`, processor send/receive overhead `o`, minimum send/receive separation `g`,
and `P` processors. `o` is time during which the processor cannot do other
work. `L` is an upper bound absent stalls; the paper's runtime analysis uses
`L` per message. Under that convention an isolated one-way delivery costs
`L + 2o`; independent messages can overlap latencies. The bottleneck endpoint's
gap, not the sum of every message's `L`, constrains sustained throughput.
At most `ceil(L/g)` messages may be in flight from any processor or to any
processor; sends stall when that capacity would be exceeded.

For these shared-memory locks, a cache-line coherence transaction is only an
*analogy* to a message. A load hitting a spinning thread's private cache is not
a fresh message; ownership acquisition, invalidation, forwarding, and refill
may require several transactions. We must not assign one `L` to every atomic
instruction or assume a completion flag costs exactly one network message.

The original paper does discuss implicit messages for remote shared-memory
references, but does not model hardware cache-coherence states or cache
capacity. A closer precedent for the required adaptation is Ramos and
Hoefler's cache-coherent communication model [3]; its architecture-specific
constants must not be transferred to this machine. LogGP [2] supplies the
long-message extension, not a ready-made cache-line handoff model.

Use effective parameters indexed by transaction type and topology:
`L_t`, `o_t`, `g_t` for same-socket/cross-socket reads, ownership transfers, and
contended RMWs. These are an explicit adaptation, not vanilla homogeneous LogP.
`P` is the number of hardware execution/communication endpoints; `N` is the
number of contending application threads. SMT and oversubscription need
separate treatment. Start with pinned, non-preempted workers, one lock, one
outstanding request per worker, finite non-blocking delegates, and a warmed
working set. Then relax those assumptions.

Keep protected-data movement `M(W, topology, reuse)` explicit, with `W` the
actual lines touched/reused rather than the allocated object size. For
independent pipelined line transfers, a candidate fit is
`d + (W - 1) gamma` for `W >= 1`, where `d` is effective transaction latency
and `gamma` effective issue/transfer spacing. A dependent pointer chain can
instead cost approximately `W d`. Neither is a universal coherence theorem;
finite outstanding capacity and bandwidth constrain the first fit. Let
`M(0) = 0`. LogGP's extra long-message bandwidth parameter is useful only if
large contiguous payload transfers actually justify that abstraction.

Other necessary parameters are delegate service costs `C_i`, requester
noncritical work `Z_i`, request/result footprint, pending queue size `n`,
effective completed batch size `b`, and scheduler/metadata working-set size.
The combiner's data may spill beyond L1; delegation preserves executor
identity, not guaranteed L1 residency. Request inputs and scheduler metadata
still move, and the combiner itself can change.

### Source-Grounded Event Graph

Represent each operation as publication -> discovery -> scheduling -> delegate
execution -> completion publication -> requester return. Record executor
election/handoff and protected-data acquisition at their actual positions.
Overlap independent requests, but preserve the total order of delegates.
This performance abstraction assumes a correct synchronization protocol; it
is not a linearizability or Rust memory-model proof.

The most important implementation details are:

| Path | Actual behavior and modeling consequence |
|---|---|
| FC-PQ publication | `fc_pq/lock.rs:109-123,275-309`: requester writes input/clears completion; only inactive nodes enter the announcement ring. Active repeat requests reuse their node. Include activation rate, not one ring enqueue per completion. Election uses a shared raw mutex with retries. |
| FC-PQ ring | `fc_pq/lock/buffer.rs:52-84,96-104,122-150`: tail `fetch_add`, capacity 64, per-slot valid flag; one drain snapshots at most 64 slots. A reserved but unpublished slot can stall the combiner. Capacity 64 does not limit the total pending PQ to 64. |
| FC-PQ scheduling | `fc_pq/lock.rs:145-255`: drain once, then at most 64 PQ-pop attempts, with completed-node buffering/removal. `H=64` is not 64 completions. A requester may republish and be served again during one pass. Let `k` count pops, `a` new announcements, `b` actual completions. |
| FC-PQ cost | `sequential_priority_queue.rs:14-61`: BinaryHeap and BTreeSet are sequential under the combiner mutex. Ordinary queue work is `O((a+k) log(n+1))` per pass, plus timing, flag checks, buffers and allocation effects. Heap growth can move `O(n)` storage on a particular insertion; steady-state/amortized cost is not a per-insertion worst-case latency bound. Dividing by `b` requires `b>0`. |
| Prefetch | `fc_pq/lock.rs:199-205` prefetches the next node's input before the current delegate. Account for possible overlap, not guaranteed elimination of a remote miss. |
| Executor return | `fc_pq/lock.rs:286-295`: the elected combiner returns from `combine()` and unlocks before testing its own completion. Its request can finish well before its call returns, and it can need multiple passes. |
| FC | `fc/lock.rs:53-108,118-138,152-170`: inactive-node head-CAS publication, full linked-list scan, completion flags and periodic stale-node cleanup. There is no `H` cap. With `n_list` retained nodes, discovery is `O(n_list)` visits per pass, or `O(n_list/b)` per completion for `b>0`, not necessarily `O(N)` per operation under saturation. Election/activation retries are additional traffic. |
| FC-Ban | `fc_ban/lock.rs:59-62,105-138,148-167`: same scan with time-based skips; skipped requests can require later passes. The penalty uses a retained-registration counter, not instantaneous waiters. Its measured interval can include skipped-node scan work, so it is not pure delegate cost. Separate eligibility-check work, requester delay and actual server idle time. |
| CC | `cc/lock.rs:23,52-86,97-121`: one tail swap per submission, predecessor-slot input/link publication, local flag waiting, sequential traversal and completion/baton stores. Default `H=64` bounds executed delegates here, unlike FC-PQ's pops; early termination on an absent next link is possible. Per-batch traversal is `O(b)` plus enqueue/handoff work and stalls. |
| CC-Ban | `cc_ban/lock.rs:61-96,145-184`: caller delays before enqueue, then CC-style execution with penalties. Its cap is 16 rather than CC's default 64. A raw CC-Ban minus CC result therefore includes a batch-policy difference, not only admission fairness. |
| MCS | `mcs.rs:78-107,124-152`: tail swap, predecessor linking, local flag spinning and successor flag handoff (or empty-queue CAS). The holder executes the CS; ordinary FIFO handoff already changes executor. A cached polling load is not a new coherence message, and constant successful protocol operations do not imply constant elapsed latency. |
| CFL | `cfl.rs:354-390,397-534,721-745`: NUMA-counter scans, variable queue shuffling, and per-core/per-NUMA service accounting at unlock. Model shuffle count and nodes visited rather than assigning a fixed message count per operation. Its fairness domain is not automatically identical to FC-PQ's per-requester credit. Both CFL and MCS use caller execution, not combining. |

All paths in the table are relative to `crates/libdlock/src/dlock2/` except
`sequential_priority_queue.rs`, which is in `crates/libdlock/src/`.
Treat zero-completion passes, election retries, inactive scans, and ring
backpressure as costs, not completed work. For aggregate batch size use
`b = total completions / total passes`, including empty passes; use ratios of
totals rather than the unweighted average of per-pass costs divided by batch.

For admission-controlled variants, a banned requester's entire cooldown is
not necessarily server idle time: other requests can execute meanwhile.
Only uncovered idle intervals enter `I_D` below. CPU spinning and requester
response delay are additional, distinct costs. Secondary variants (DSM,
FC-SL, Ticket/CLH, ShflLock, U-SCL) can extend the event table after the core
model is validated; they should not inherit FC's or CC's budget semantics
without inspecting their own execution paths.

### Latency Bounds Versus Throughput Estimates

Build an event DAG for a finite interval of `B` completed operations, including
startup/drain when relevant. In a homogeneous calibrated abstraction let:

- `CP` be its longest causal path, including required communication;
- `W_p` be non-overlappable CPU occupancy at endpoint `p`, including charged
  local work and endpoint communication overhead;
- `v_p_send`, `v_p_recv` be small-message counts at that endpoint;
- `r_l` be serialized transactions at a hot cache line, each with effective
  occupancy `a_l` (an added coherence constraint).

For the inequality below, durations and service occupancies are exact costs
of a deterministic abstract machine, or guaranteed minima. Inserting
empirical means gives a predictive approximation, not a hardware lower bound.
In particular, classical LogP's upper-bound `L` is not a measured minimum
latency from which a physical lower bound follows.

Within that abstraction, the interval time has the necessary lower bound

```text
T >= max(CP,
         max_p W_p,
         max_p max((v_p_send - 1)_+, (v_p_recv - 1)_+) g,
         max_l r_l a_l)
```

Here `x_+ = max(x, 0)`. Include capacity stalls and transfer dependencies in the
DAG; topology-specific service/bandwidth constraints refine the homogeneous
gap term. Correspondingly `X = B/T` has an upper bound, not an exact prediction.
Those bottlenecks need not all be simultaneously attainable. Counting all
messages times `L+2o` as a throughput cost would incorrectly discard overlap.

For an interpretable *serialized-path approximation*, write time per operation:

```text
t_D ~= C_D + d_D + [A_D + p_D (M_D + K_D)] / b_D + I_D
t_H ~= C_H + h_H + M_H
```

`D` is delegation; `H` is a handoff lock. `C` is service with the chosen
operation mix and warm executor. `d_D` is per-operation scheduling, publication/
completion and discovery cost exposed on the serialized path. `A_D` is
per-pass election/administration excluding migration. `p_D` is the fraction
of passes changing executor; `M_D` is protected-data reacquisition conditional
on that change, and `K_D` is additional scheduler-state migration. `I_D`
is exposed admission/empty-server idle time per completion. `h_H` includes
handoff coordination and any fairness policy. `M_H` is actual data movement
per handoff operation, including zero or smaller costs for local reuse.

This additive estimate is not an upper bound: resource contention can make it
optimistic, and residual costs require calibration. A transferred input miss
must not be counted both inside measured `C` and again in `d`; likewise `M`
must not be added to service times that already include the same cold misses.
Check the DAG/resource bounds independently. Throughput is approximately
`1/t` only in the saturated regime with these overheads represented.

In the deliberately simplified matched-work case `C_D=C_H`, `M_D=M_H=M`,
`p_D=1`, `I_D=0`, abbreviating other subscripts gives:

```text
within this approximation, delegation wins iff d - h < (1 - 1/b) M - (A + K)/b
                 iff b (M + h - d) > M + A + K.
```

If `M+h-d > 0`, the crossover is `b > (M+A+K)/(M+h-d)`; otherwise no
batch size wins under these assumptions. This is a testable conditional
result: increasing batch size amortizes movement and administration, but not
per-request scheduling/communication. Tiny warm-state CSs can favor ordinary
locks, while costly cross-core state movement can favor combining. Large
payloads, cold PQ metadata, or a congested combiner can reverse that advantage.
Do not substitute the source constant 64 for measured `b`.

### The Cost of Fairness: Use Matched Deltas

Compare `FC-PQ - FC` and `CFL - MCS`, not just FC-PQ against CFL:

```text
Delta_D ~= Delta C + Delta d
           + Delta{[A + p(M+K)]/b} + Delta I
Delta_H ~= Delta C + Delta h + Delta M.
```

Within an epoch, if all protected state is accessed only by its combiner,
changing service order cannot by itself transfer that state to another
executor. This is an ownership argument, not a guarantee of identical cache
misses: order can change inputs, state accesses, eviction, and service cost.
Fair scheduling can also change batch occupancy and executor-change frequency.

MCS already hands data between owners. CFL does not uniquely introduce the
whole migration cost; its marginal cost is policy work plus the *difference*
in movement/locality relative to MCS. Likewise, an FC-PQ pass does not have
the same discovery machinery as an FC pass. The deltas above are hypotheses
to explain, not identities equating all observed slowdown to heap operations.

Control the operation mix. With deterministic class cost `C_i` and service-time
share `f_i`, an overhead-free saturated server has
`X_i = f_i/C_i` and `X = sum_i f_i/C_i`. Usage fairness changes `f_i` and hence
completed-operation throughput even with zero scheduling cost. For two
threads with costs 1 and 9, one request each per round gives `X=1/5`, while
equal service-time shares give `X=5/9`. These are illustrative operations per
normalized time unit, not measurements. Report per-class completions, useful work, service
shares, and total throughput; do not interpret a changed workload mix as
pure synchronization overhead.

### Fairness Theorem and Its Implementation Gap

**Ideal exact-min theorem.** Fix a continuously eligible, backlogged cohort.
At each service boundary, serve a thread with minimum cumulative credit `U_i`
and add a nonnegative charge `c <= C_max`. Do not reset or rewrite credit.
If the initial spread is `D_0`, then at every service boundary:

```text
max_i U_i - min_i U_i <= max(D_0, C_max).
```

Proof: let the previous minimum be `m`. All other credits remain in
`[m, m+D]`, while the updated credit lies in `[m, m+C_max]`; the new minimum
cannot decrease. Induction preserves `D=max(D_0,C_max)`. Equal initial credits
give the familiar `C_max` bound. The eligibility-window variant may choose
any credit at most `m+delta`, giving `max(D_0,delta+C_max)` by the same argument.
This latter statement is about an ideal policy, not implemented FC-EW.

This is a service-credit bound, not an `O(C_max)` response-time bound, a JFI
guarantee for arbitrary windows, or an unconditional bound on all threads'
lifetime service. Finite-window service differences subtract initial credits;
even bounded cumulative spread can give twice that bound across two
endpoints. Backlogged in the abstract scheduler means a request is eligible
at every selection; fast resubmission by real synchronous callers is not
automatically equivalent.

The existing FC-PQ needs a separate refinement argument:

1. `fc_pq/lock.rs:157-162` initializes zero-usage arrivals to
   `total_usage/total_served`, the mean *per-operation charge*, not the
   cohort's current cumulative virtual time. Two old threads at credit 100
   after 200 unit-cost services admit a newcomer at credit 1. The current
   active-credit spread is 99 although `C_max=1`. Thus the fixed-cohort theorem
   cannot be quoted for changing membership. Define fairness relative to
   activation/backlog intervals before choosing a newcomer policy.
2. `fc_pq/lock.rs:193-197` attempts to clamp credit only after a node is
   popped. Under the valid min-PQ invariant, the popped credit is already no
   greater than the next minimum, so `min(current, next_min)` changes nothing.
   It cannot force an unpopped high-credit node to be selected within eight
   passes. No additional starvation or wall-clock bound follows from this
   threshold; credit-reset policies, if later introduced, would need their
   own proof relating virtual credit to actual delivered service.
3. Announcements are drained only at pass entry. A lower-credit request can
   be published but not yet visible to the local PQ; buffering/deactivation
   and reactivation further affect the eligible set. Any bound needs an
   explicit discovery-delay term and admission/progress assumptions.
4. The charge at `fc_pq/lock.rs:209-217` brackets input access, delegate
   execution and result write. It is not exactly the benchmark's inner
   `hold_time`, nor the requester's CPU consumption. Bound the charged
   quantity and state its relationship to application service separately.
   Counter wraparound and unbounded preemption are excluded from the ideal
   theorem, not silently handled by it.

Record these as proof obligations, not algorithm changes in this worktree.
The old README's unconditional `|U_i-U_j| <= C_max` is insufficient evidence.

### Response Time and Batch Tradeoff

For a waiter, decompose invocation-to-return time into publication/admission,
discovery, scheduler waiting, own service, and completion observation.
For a caller that combines, add the work it must finish before returning even
after its own request is complete. These components may overlap across
different callers; do not sum them to estimate aggregate throughput.

Larger batches reduce `(A+p(M+K))/b` but can lengthen a combiner caller's
post-completion delay. Only with bounded service, discovery, queue work and
scheduling delays can a pass-time bound be derived. `H=64` alone is
insufficient: ring draining waits for publication, acquisition retries can
continue, and delegates are non-preemptive. A long operation can delay any
later eligible request by its residual service time.

For a stationary closed workload with `N` callers and one outstanding request
per caller, use the response-time law `N = X (E[R] + E[Z])` as a consistency
check, with operation-weighted mean response/noncritical times. It is not a
p99 formula. Equal usage shares do not imply equal operation counts, equal
response times, or balanced combiner CPU work.

### Validation Proposal: Approval Required Before Implementation

Proceed in this order, without fitting a new arbitrary constant for each lock:

1. **Protocol accounting.** Derive per-epoch counts from each implementation:
   completions, scans/pops, announcements/reactivations, failed elections,
   executor changes, and empty/idle intervals. Draw dependency graphs and
   separate CPU work from coherence traffic. Do not infer messages by counting
   source loads. Acceptance: every cost term has a source event and units.
2. **Independent calibration.** Measure cache-line handoff latency, dependent
   versus independent transfers, hot-line RMW throughput, publication and
   completion paths, warm delegate cost, and local PQ cost by size. Repeat
   same-socket/cross-socket; preserve CPU placement, frequency/TSC convention,
   memory placement and input footprint. Acceptance: one parameter set per
   topology/transaction class, not a per-result fitted residual.
3. **Minimal instrumented comparison.** Begin with FC, FC-PQ BinaryHeap, MCS
   and CFL; use CC/CC-Ban/FC-Ban to distinguish discovery and admission terms.
   Measure actual `b`, change probability `p`, metadata footprint and exposed
   stalls. Sweep threads, non-CS work, CS heterogeneity, protected footprint,
   request footprint and socket placement. Batch-budget sweeps require an
   approved code/config change; they are not an existing CLI capability.
4. **Held-out prediction.** Calibrate on a subset, then predict crossover
   direction and marginal fairness costs on unseen footprints/thread counts.
   Plot predicted versus observed per-operation time and explain residuals.
   At fixed mix/placement, varying effective `b` should expose an amortized
   `1/b` component until another bottleneck dominates. Failure to predict
   held-out trends rejects the model or its assumed regime; do not hide it
   by refitting every point.
5. **Fairness separately.** Track actual per-thread delivered service and
   scheduler credit at selection boundaries. Cover steady cohorts,
   intermittent callers, newcomer bursts, high-cost requests, delayed
   publication and reactivation. Use counterexamples to refine the theorem,
   not just final JFI close to one. Scope progress assumptions explicitly.

Expected deliverables after approval: protocol/cost table, scoped theorem
with proof or counterexamples, crossover figure, matched fairness-cost
decomposition, and response-time/combiner-work tradeoff plot. Start with
these implemented mechanisms before using the model to justify FC-EW.

**Measurement caveats found in the current source:**

- `fc_pq/lock.rs:129-131,209,258-263` reuses `begin` at each delegate before
  adding `end-begin` to `combiner_time_stat`; this is not a full-pass timer
  when a pass serves requests. Do not calibrate total epoch work from it.
- `src/benchmark/dlock2/proportional_counter.rs:106-109` labels a request
  by whether its requester executes its own delegate. That is useful, but
  not a complete record of which callers spent time combining other requests.
- `src/benchmark/dlock2/counter_common.rs:228-241` records both
  `num_acquire` (operations) and `loop_count` (configured counter-work units).
  Use the appropriate denominator. Inner delegate `hold_time` and outer PQ
  accounting have different boundaries; retain both definitions.
- Generic L1/LLC miss counts cannot by themselves identify ownership
  transfers, protected-state traffic, and scheduler/request traffic. Validate
  the decomposition using controlled accesses and suitable hardware events,
  with their platform-specific limitations stated.

### Verification Boundary

The proofs above are conditional mathematical arguments, not a concurrency
verification of the Rust implementations. The worktree-local scratch script
`.worktree/logp_checks.py` checked 14,508 one-step integer transitions for
2–4 participants (exact-min, eligibility-window, and larger initial spread),
plus 6,912 exact-rational batch-crossover cases and the newcomer/mix examples.
These finite checks passed; they supplement, not replace, the induction proof.
They cannot establish hardware costs, Rust correctness, or progress.

The prescribed `devenv shell -- python3 .worktree/logp_checks.py` failed during
environment evaluation because `dotenv.resolved` has no defined value.
The standard-library-only arithmetic check was instead run successfully with
host `python3`; no environment files were changed. No lock tests, performance
benchmarks or new empirical results are claimed. Instrumentation fixes and
experiments remain subject to plan approval.

### Primary References

1. Culler et al., *LogP: Towards a Realistic Model of Parallel Computation*,
   PPoPP 1993, [DOI](https://doi.org/10.1145/155332.155333);
   [Berkeley report](https://www2.eecs.berkeley.edu/Pubs/TechRpts/1992/6262.html)
   and [full paper](https://people.eecs.berkeley.edu/~kubitron/cs258/handouts/papers/logp.pdf).
   Section 3 defines parameters/capacity; section 3.2 discusses shared-memory
   references as implicit messages. Neither specifies our coherence mapping.
2. Alexandrov et al., *LogGP*, SPAA 1995,
   [DOI](https://doi.org/10.1145/215399.215427);
   [expanded UCSB report](https://cs.ucsb.edu/research/tech-reports/1995-09),
   *LogGP: Incorporating Long Messages into the LogP Model*.
   Adds `G`, the per-byte gap for long messages.
3. Ramos and Hoefler, *Modeling Communication in Cache-Coherent SMP Systems—
   A Case-Study with Xeon Phi*, HPDC 2013,
   [author publication record](https://spcl.inf.ethz.ch/Publications/index.php?pub=162)
   and [paper](https://spcl.inf.ethz.ch/Publications/.pdf/hoefler-ramos-hpdc13-cc_modeling.pdf).
   Precedent for explicit cache-line state/traffic modeling, not calibration
   data for our hardware.
