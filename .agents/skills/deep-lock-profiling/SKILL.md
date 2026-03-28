---
name: deep-lock-profiling
description: Deep performance profiling workflow for synchronization-heavy systems. Use when explaining why one lock or concurrent design is faster than another across workloads or machines, especially when cache locality, core migration, fairness, combiner behavior, or scheduler effects may dominate.
---

# Deep Lock Profiling

## Purpose

Use this skill to explain **why** a synchronization primitive wins or loses, not just **whether** it is faster. It is designed for lock-heavy and delegation-heavy systems where throughput alone is misleading and the real causes may involve cacheline ownership, handoff migration, fairness bookkeeping, combiner concentration, scheduler interference, or NUMA placement.

This skill assumes a workflow built around four layers:

1. **Benchmark-level decomposition first**
2. **Normalized PMU counters second**
3. **Hotspot / cache-to-cache attribution third**
4. **Scheduler tracing last**

Never start with raw perf traces before classifying the performance gap from workload results.

## When to Use

- Comparing locks, wait strategies, or synchronization designs
- Explaining per-workload performance differences
- Explaining cross-machine differences for the same workload
- Investigating cache locality vs fairness tradeoffs
- Investigating delegation locks, queue locks, flat combining, MCS/CLH/Ticket/CFL/ShflLock-style behavior
- Diagnosing whether a slowdown comes from bookkeeping, coherence traffic, or scheduler effects

## Core Principle

Treat each performance gap as one of these causes until the evidence says otherwise:

- **Fairness / service-order effect**
- **Bookkeeping overhead**
- **Cache / coherence migration cost**
- **Combiner penalty or concentration**
- **Scheduler / machine noise**

Do not mix these explanations prematurely.

## Measurement Ladder

### Stage 1 — Benchmark Metrics First

Start from the benchmark harness outputs, not perf.

Collect and interpret:

- throughput / completed acquires
- fairness metrics (for example JFI, normalized share)
- role split metrics (combiner vs waiter latency if available)
- hold time / combine time if available
- workload knobs: CS length, non-CS length, queue mix, array size, access pattern, thread count, socket placement

Use these to classify the gap:

- High throughput + poor fairness + bad waiter tails → likely **service-order imbalance**
- Similar fairness + higher cost → likely **bookkeeping overhead**
- Throughput collapses when access becomes random / footprint grows → likely **cache migration / locality loss**
- Variance across identical runs with low algorithmic signal → likely **scheduler or placement noise**

### Stage 2 — Perf Stat, But Normalize Everything

Run `perf stat` only after Stage 1.

Prefer interpreting:

- `cycles / acquire`
- `instructions / acquire`
- `L1 misses / acquire`
- `LLC misses / acquire`
- `dTLB misses / acquire`
- `branch-misses / acquire`
- `cpu-migrations / second`
- `context-switches / second`

**Never compare raw totals across different locks** when throughput differs. Faster locks naturally retire more instructions and touch more memory in aggregate.

### Stage 3 — Hotspots and Ownership Attribution

Escalate only after `perf stat` suggests a cause.

- Use `perf record/report` for code-path attribution
- Use `perf c2c` for cacheline ping-pong, false sharing, HITM, remote HITM
- Use `perf mem` for sampled memory-side evidence (mem level, snoop, blocked)

Use hotspot attribution to answer:

- Is the cost in fairness bookkeeping data structures?
- Is the cost in lock handoff or waiter polling?
- Is the protected shared data migrating across cores?
- Is the hot cacheline the lock metadata or the payload?

### Stage 4 — Scheduler Tracing Only When Needed

Use `perf sched` only when pinned runs still show noise, migrations, unexplained variance, or cross-machine instability.

Ask:

- Are threads being moved despite affinity?
- Are wakeups delayed?
- Is one thread repeatedly preempted while holding the hot role?
- Is SMT or socket spread changing runqueue delay?

## Fixed Interpretation Rules

Apply these defaults unless the evidence contradicts them.

### Delegation Fairness vs Unfair Delegation

If unfair delegation and fair delegation have:

- similar miss/acquire
- similar locality-sensitive behavior
- but fair delegation has higher instructions/acquire or branch cost

then the gap is primarily **scheduler / fairness bookkeeping overhead**, not lost locality.

### Queue / Fair-Handoff Locks

If fair queue locks show:

- higher LLC misses/acquire
- more HITM / remote HITM
- stronger sensitivity to socket spread

then the gap is primarily **handoff-induced migration / coherence traffic**.

### Combiner Penalty

If throughput is good but combiner latency is much worse than waiter latency, or one role dominates combine time, then the issue is **combiner concentration**, not necessarily locality failure.

### Banning / Idling Variants

If fairness improves but instructions/acquire and idle time rise without stronger cache symptoms, the cost is likely **non-work-conserving fairness policy**, not migration.

## Workload Selection

Do not start from the full matrix. Pick a few **witness workloads** per machine.

Recommended witnesses:

1. **Main tradeoff witness**
   - heterogeneous critical sections
   - saturated contention

2. **Locality witness**
   - data structure larger than L1
   - random access to defeat prefetch help
   - sequential control run for comparison

3. **Pure overhead witness**
   - uniform or tiny CS
   - same machine placement

4. **Optional machine-sensitivity witness**
   - packed vs spread sockets
   - SMT on/off if feasible

## Cross-Machine Rules

When comparing machines:

- record topology first (`lscpu`, NUMA layout, SMT state, kernel, perf event support)
- use the same witness workloads, not the whole experiment suite
- keep event sets architecture-specific if needed
- compare **normalized** metrics, not raw event totals
- expect top-down and some PMU names to vary by microarchitecture

Do not assume that a result means the same thing across Intel and AMD without checking event semantics.

## Output Layout

Store profiling artifacts under:

```text
profile/<arch>/<workload>/<config>/
```

Recommended structure:

```text
profile/
  <arch>/
    machine.md
    <workload>/
      <config>/
        benchmark/
        perf-stat/
        perf-record/
        perf-c2c/
        perf-sched/
        summary.md
```

### `machine.md`

Record:

- CPU model / stepping
- sockets / cores / threads
- NUMA topology
- SMT state
- kernel version
- perf event availability
- pinning / placement notes

### `summary.md`

Every run summary should answer exactly three things:

1. **Observation**
2. **Evidence**
3. **Conclusion**

Example:

> FC-PQ is 8% slower than FC but preserves fairness.
> LLC misses per acquire stay flat while instructions per acquire rise.
> Therefore the loss comes from fairness bookkeeping, not loss of combiner locality.

## Command Patterns

### Perf Stat Baseline

```bash
perf stat -d -d -d -I 1000 \
  -e cycles,instructions,context-switches,cpu-migrations \
  -- <cmd>
```

Then add the architecture-appropriate cache and TLB events.

### Topdown

```bash
perf stat --topdown -I 1000 -- <cmd>
```

Use only when the workload is actually CPU-bound.

### Scheduler Diagnosis

```bash
perf sched record -- <cmd>
perf sched timehist -M -w
perf sched latency
```

### Cache-to-Cache

```bash
perf c2c record --all-user -- <cmd>
perf c2c report --stdio
```

### Memory Sampling

```bash
perf mem record --all-user --ldlat 30 -- <cmd>
perf mem report
```

## Questions to Ask in Order

1. What exactly got slower: throughput, fairness, tail latency, or one role?
2. Did the slowdown appear only under heterogeneity, random access, or socket spread?
3. Does normalization show more work per acquire, or more stall per acquire?
4. Is the hot cost in bookkeeping structures, handoff state, or payload data?
5. Are scheduler delays or migrations large enough to change the story?

## Anti-Patterns

- Starting with `perf record` before understanding the benchmark result
- Comparing raw PMU counts across locks with different throughput
- Claiming locality loss from L1 misses alone on metadata-heavy locks
- Ignoring the random-vs-sequential control in locality experiments
- Mixing fairness-inside-delegation and fairness-inside-handoff paradigms into one explanation
- Blaming the OS before checking lock-internal bookkeeping or combiner behavior

## Expected Deliverable

When using this skill, produce:

1. a small set of witness workloads
2. a benchmark-first classification of each performance gap
3. a normalized PMU interpretation
4. deeper attribution only where necessary
5. a recorded artifact tree under `profile/<arch>/...`

The goal is not just to say which lock wins. The goal is to explain, with evidence, **why** it wins on this workload and **why** that explanation may change on another machine.
