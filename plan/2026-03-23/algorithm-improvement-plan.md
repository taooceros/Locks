# Algorithm Improvement Plan: Beyond FC-PQ

Date: 2026-03-23

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
