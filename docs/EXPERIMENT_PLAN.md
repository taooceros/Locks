# Experiment Plan: Demonstrating Delegation Lock Performance

This document specifies the full experiment suite for validating the core claims
of the usage-fair delegation locks paper. Each experiment group has a hypothesis,
exact configuration, metrics, and mapping to paper figures.

---

## Common Parameters

| Parameter | Value | Notes |
|-----------|-------|-------|
| Warmup | 2s | `--warmup 2` (default) |
| Trials | 3 | `--trials 3` for statistical confidence |
| Duration | 15s | `--duration 15` for throughput experiments |
| Duration (latency) | 5s | `--duration 5` for `--stat-response-time` (memory) |
| Output | `visualization/output` | Default |

### Machines

| Machine | Cores | Threads | Architecture | NUMA | Role |
|---------|-------|---------|-------------|------|------|
| Intel Xeon Gold 6438M | 64 | 128 | Sapphire Rapids | 2 nodes (node0: 0-31,64-95; node1: 32-63,96-127) | Primary (saturn) |
| AMD EPYC (TBD) | TBD | TBD | Zen 3/4 | TBD | Cross-vendor validation |
| Cloudlab c6525-100g | 32 | 64 | AMD EPYC 7543 | 2 nodes | Fallback |

**Saturn details:** 2 sockets × 32 cores × 2 HT = 128 threads. L1d=48KB/core,
L2=2MB/core, L3=60MB/socket (shared). Cross-socket interconnect: UPI.

**AMD TBD:** Need at least one AMD machine for cross-vendor validation.
Different L3 topology (CCX/CCD) and interconnect (Infinity Fabric) may
change the delegation vs traditional lock crossover point. Fill in specs
once machine access is confirmed. Run `lscpu`, `numactl --hardware`, and
`lstopo` on first login to populate this section.

### Lock Groups

Referenced by name throughout this document.

```
DELEGATION_UNFAIR = FC, CC, DSM
DELEGATION_FAIR   = FCBan, CCBan, fc-pq-b-heap
TRADITIONAL       = MCS, mutex, spin-lock, USCL
BASELINES_C       = fc-c, cc-c, shfl-lock, shfl-lock-c
ALL               = DELEGATION_UNFAIR ∪ DELEGATION_FAIR ∪ TRADITIONAL ∪ BASELINES_C
```

CLI lock target names (for `-l` flag):
`fc, fc-ban, cc, cc-ban, dsm, fc-sl, fc-pq-b-tree, fc-pq-b-heap, mutex, spin-lock, uscl, fc-c, cc-c, mcs, shfl-lock, shfl-lock-c`

### Aliases (for experiment scripts)

```nu
alias dlock2 = target/release/dlock d-lock2

let all_threads = "4,8,16,32,64,128"
let mid_threads = "8,16,32,64"

let delegation_unfair = "fc,cc,dsm"
let delegation_fair = "fc-ban,cc-ban,fc-pq-b-heap"
let traditional = "mcs,mutex,spin-lock,uscl"
let baselines_c = "fc-c,cc-c,shfl-lock,shfl-lock-c"

let core_locks = "fc,fc-pq-b-heap,fc-ban,mcs,mutex"
let all_locks = $"($delegation_unfair),($delegation_fair),($traditional),($baselines_c)"
```

---

## Group 1: CS Ratio Sweep

**Hypothesis:** FC-PQ maintains JFI near 1.0 regardless of CS heterogeneity ratio.
Traditional locks and unfair delegation degrade as the ratio widens.

**Claim validated:** Contribution #3 — delegation breaks the fairness-performance
tradeoff.

### Configuration

| CS Pair | Ratio | Description |
|---------|-------|-------------|
| 1000,1000 | 1:1 | Homogeneous (control) |
| 1000,3000 | 1:3 | Mild heterogeneity (current baseline) |
| 1000,10000 | 1:10 | Moderate heterogeneity |
| 100,3000 | 1:30 | High heterogeneity |
| 100,10000 | 1:100 | Extreme heterogeneity |

```nu
for cs in ["1000,1000" "1000,3000" "1000,10000" "100,3000" "100,10000"] {
    dlock2 counter-proportional -t $all_threads --cs $cs --non-cs 0 -d 15 --trials 3
}
```

### Locks

ALL

### Metrics

- Total throughput (loop_count sum) per (ratio, thread_count)
- JFI per (ratio, thread_count)
- Per-thread normalized share

### Expected Results

- FC, CC, DSM: JFI degrades with wider ratios (long-CS threads monopolize)
- MCS, Mutex: JFI also degrades (long-CS holders block others longer)
- **FC-PQ: JFI stays >0.99 across all ratios** — priority queue rebalances
- FCBan: JFI stays high but throughput drops more than FC-PQ (non-work-conserving)
- USCL: JFI stays high, throughput remains low

### Paper Figures

- **Fig 2:** JFI vs CS ratio (line plot, one line per lock). X-axis: ratio (1:1 to 1:100). Y-axis: JFI. Shows FC-PQ flat near 1.0 while others degrade.
- **Fig 3:** Throughput vs thread count (one panel per ratio). Shows FC-PQ gap from FC stays small even at 1:100.
- **Table:** JFI × throughput scatter data for Pareto frontier plot.

---

## Group 2: CS Length Scalability Crossover

**Hypothesis:** Delegation's throughput advantage over traditional locks grows with
CS length because shared data stays in the combiner's L1. The FC-PQ fairness
overhead (O(log N) priority queue ops) is constant, not multiplicative.

**Claim validated:** Delegation's L1 locality advantage is proportional to CS length.

### Configuration

Uniform CS (all threads same length) — this isolates the lock overhead from fairness effects.

```nu
for cs in [1 10 100 1000 5000 10000 50000] {
    dlock2 counter-proportional -t 32 --cs $cs --non-cs 0 -d 15 --trials 3 \
        -l fc,fc-pq-b-heap,fc-ban,mcs,mutex,spin-lock \
        --file-name $"cs-crossover-($cs)"
}
```

### Locks

FC, FcPqBHeap, FCBan, MCS, Mutex, SpinLock

### Metrics

- Throughput vs CS length (single thread count = 32)

### Expected Results

- CS=1: Delegation overhead (publication list scan, combiner loop) may lose to MCS
- CS=10-100: Delegation starts winning — amortized overhead < migration cost
- CS=1000+: Delegation dominates — shared data stays L1-hot
- **FC vs FC-PQ gap stays constant** in absolute terms as CS grows (overhead is per-pass, not per-iteration)
- SpinLock/Mutex: collapse under contention regardless of CS length

### Paper Figures

- **Fig 4:** Throughput vs CS length (log-log scale). One line per lock. Shows delegation crossover and constant FC-PQ overhead.

---

## Group 3: Non-CS Sweep (Contention Levels)

**Hypothesis:** Delegation advantage is largest under high contention (non-CS=0)
and narrows as contention decreases. Fair delegation overhead becomes negligible
under low contention.

**Claim validated:** Delegation is most beneficial under high contention — the
regime where fairness matters most.

### Configuration

```nu
let simple_cs = "1000,3000"

for non_cs in [0 10 100 1000 10000 100000] {
    dlock2 counter-proportional -t $all_threads --cs $simple_cs --non-cs $non_cs -d 15 --trials 3 \
        -l fc,fc-pq-b-heap,fc-ban,mcs,mutex,uscl
}
```

### Locks

FC, FcPqBHeap, FCBan, MCS, Mutex, USCL

### Metrics

- Throughput per (non-CS, thread_count)
- JFI per (non-CS, thread_count)

### Expected Results

- Non-CS=0: Maximum contention — delegation 2-5x over MCS, JFI differences most pronounced
- Non-CS=100000: Low contention — all locks converge in throughput; JFI differences shrink because fewer concurrent waiters
- FC-PQ overhead (vs FC) smallest at low contention (fewer nodes in PQ)

### Paper Figures

- **Fig 5:** Throughput vs thread count, one panel per non-CS level. Shows delegation advantage region.

---

## Group 4: Response Time Distributions

**Hypothesis:** Unfair delegation has bimodal response time distribution (combiner
penalty). Fair delegation has more uniform distribution. This motivates the
combiner-as-scheduler design.

**Claim validated:** Contribution #4 — combiner response-time analysis.

### Configuration

```nu
# Single-addition (minimal CS) — focuses on lock overhead
dlock2 counter-proportional -t 8,16,32 --cs 1 --non-cs 0 -d 5 --trials 3 \
    --stat-response-time \
    -l fc,fc-pq-b-heap,fc-ban,cc,cc-ban,mcs \
    --file-name "latency-cs1"

# Proportional CS — shows fairness effect on response time
dlock2 counter-proportional -t 8,16,32 --cs 1000,3000 --non-cs 0 -d 5 --trials 3 \
    --stat-response-time \
    -l fc,fc-pq-b-heap,fc-ban,cc,cc-ban,mcs \
    --file-name "latency-cs1000-3000"
```

### Locks

FC, FcPqBHeap, FCBan, CC, CCBan, MCS

### Metrics

- Response time percentiles: p50, p95, p99, p99.9
- Split by combiner vs waiter role
- CDF data export

### Expected Results

- FC: Bimodal — combiners see sum-of-all-CS latency, waiters see only their own
- FC-PQ: Less bimodal — PQ overhead distributes more evenly
- CC/CCBan: FIFO ordering gives more predictable waiter latency
- MCS: Unimodal but higher baseline (cross-core migration on every handoff)

### Paper Figures

- **Fig 1 (motivation):** CDF plot for FC at 32T — combiner vs waiter overlay showing bimodality.
- **Fig 6:** p99 response time bar chart across locks at 32T. Split bars for combiner/waiter.

---

## Group 5: Asymmetric Contention

**Hypothesis:** FC-PQ rebalances hold-time even when threads contend at different
rates (hot threads arriving frequently, cold threads arriving rarely).

**Claim validated:** Fairness under real-world access patterns where not all
threads contend equally.

### Configuration

The harness cycles `(cs, non_cs)` pairs across threads. With `--cs 1000,1000
--non-cs 0,10000`, thread 0 gets (cs=1000, non_cs=0), thread 1 gets
(cs=1000, non_cs=10000), thread 2 gets (cs=1000, non_cs=0), etc.

```nu
dlock2 counter-proportional -t 8,16,32,64 --cs 1000,1000 --non-cs 0,10000 -d 15 --trials 3 \
    -l fc,fc-pq-b-heap,fc-ban,mcs,uscl \
    --file-name "asymmetric-contention"
```

### Locks

FC, FcPqBHeap, FCBan, MCS, USCL

### Metrics

- Per-thread normalized share (hold_time / mean)
- JFI
- Per-thread throughput (loop_count)

### Expected Results

- FC: Hot threads (non-CS=0) dominate hold-time — normalized share >>1.0 for hot, <<1.0 for cold
- MCS: Similar — hot threads acquire more often
- **FC-PQ: Normalized shares closer to 1.0** — the priority queue compensates for contention frequency differences
- USCL: Also fair, but at much lower throughput

### Paper Figures

- **Per-thread bar chart:** Normalized share for each thread, colored by hot/cold.
  Shows FC-PQ has uniform bars while FC has skewed.

---

## Group 6: Concurrent Hash Map (NEW CODE REQUIRED)

**Hypothesis:** Fair delegation bounds lookup tail latency even with long-CS scan
threads, while maintaining high aggregate throughput because the hash map stays
in the combiner's L1.

**Claim validated:** End-to-end application demonstrating real-world fairness impact.

### Data Structure

```rust
struct HashMapState {
    map: HashMap<u64, Vec<u8>>,   // 10K entries, 64-byte values
}
```

### Operations

| Operation | Thread Role | CS Length | Description |
|-----------|-----------|-----------|-------------|
| Get | Lookup | Short (~100ns) | `map.get(&key)` + read value |
| Put | Lookup | Medium (~500ns) | `map.insert(key, value)` |
| Scan | Scanner | Long (~5-50us) | Iterate N entries, compute aggregate |

### Thread Mix

- N-2 threads: lookup (90% get, 10% put)
- 2 threads: scan (iterate 100-1000 entries per scan)
- Zipfian key distribution (theta=0.99)

### Configuration

```nu
dlock2 hashmap -t 8,16,32,64 --scan-threads 2 --scan-size 100,500,1000 -d 15 --trials 3 \
    -l fc,fc-pq-b-heap,fc-ban,mcs,mutex \
    --stat-response-time
```

### Locks

FC, FcPqBHeap, FCBan, MCS, Mutex

### Metrics

- Per-operation-type throughput (gets/s, puts/s, scans/s)
- Per-operation-type response time: p50, p99, p99.9
- Focus: **lookup p99 under varying scan load**

### Expected Results

- MCS/Mutex: Scan threads hold lock for microseconds → lookup p99 explodes
- FC (unfair): Scan threads get proportional hold-time → moderate lookup starvation
- **FC-PQ: Scan threads throttled by priority queue** → lookup p99 bounded
- FC-PQ throughput close to FC because hash map stays L1-hot in combiner

### Paper Figures

- **Fig 7a:** Lookup p99 latency vs thread count (grouped bar by lock). FC-PQ bars stay low.
- **Fig 7b:** Total throughput with operation breakdown (stacked bar). Shows FC-PQ doesn't sacrifice aggregate throughput for fairness.

### Implementation Notes

- New subcommand: `DLock2Experiment::HashMap { scan_threads, scan_size, ... }`
- New file: `src/benchmark/dlock2/hashmap.rs`
- Delegate closure receives enum: `HashMapOp::Get(key)`, `HashMapOp::Put(key, value)`, `HashMapOp::Scan(start, count)`
- Pre-populate in `start_benchmark` before spawning threads
- Track operation type in response time records (extend Records or use separate vectors)

---

## Group 7: Producer-Consumer Log (NEW CODE REQUIRED)

**Hypothesis:** FC-PQ prevents the consumer's long CS from starving producers,
while keeping the log buffer L1-hot in the combiner.

**Claim validated:** Write-dominated workload with asymmetric CS — a common pattern
in databases, event systems, and logging frameworks.

### Data Structure

```rust
struct LogBuffer {
    buffer: VecDeque<LogEntry>,  // circular buffer
}

struct LogEntry {
    timestamp: u64,
    payload: [u8; 56],          // 64 bytes total with timestamp
}
```

### Operations

| Operation | Thread Role | CS Length | Description |
|-----------|-----------|-----------|-------------|
| Append | Producer (N-1) | Short (~100ns) | Push one LogEntry |
| Drain | Consumer (1) | Long (K×50ns) | `drain(..K)`, K=100-500 |

### Configuration

```nu
dlock2 log-buffer -t 8,16,32 --drain-batch 100,250,500 -d 15 --trials 3 \
    -l fc,fc-pq-b-heap,fc-ban,mcs,mutex
```

### Locks

FC, FcPqBHeap, FCBan, MCS, Mutex

### Metrics

- Producer throughput: appends/sec per producer thread
- Consumer throughput: entries drained/sec
- Per-thread hold-time share
- JFI

### Expected Results

- MCS/Mutex: Consumer's long drain blocks all producers → producer throughput collapses
- FC (unfair): Consumer gets disproportionate hold-time
- **FC-PQ: Consumer is throttled, producers get fair share** — total throughput stays high because buffer is L1-resident
- As drain-batch K increases, the unfairness gap widens

### Paper Figures

- **Fig 8:** Producer throughput (appends/sec) vs drain batch size. One group per lock. Shows FC-PQ maintains producer throughput.

### Implementation Notes

- New subcommand: `DLock2Experiment::LogBuffer { drain_batch, ... }`
- New file: `src/benchmark/dlock2/log_buffer.rs`
- Thread 0 is always the consumer (drain); threads 1..N are producers (append)
- Delegate closure: `LogOp::Append(entry)` or `LogOp::Drain(batch_size)`
- Use `VecDeque::drain(..min(K, len))` to avoid panicking on underflow

---

## Group 8: Queue & Priority Queue (Existing Benchmarks)

**Hypothesis:** Delegation locks outperform traditional locks on sequential data
structures (queue, priority queue) because the data structure stays L1-hot.

**Claim validated:** Delegation advantage extends to realistic data structure
operations, not just synthetic counters.

### Configuration

```nu
# Queue (LinkedList)
dlock2 queue -t 4,8,16,32,64 -d 15 --trials 3 \
    -l fc,fc-pq-b-heap,fc-ban,cc,cc-ban,mcs,mutex

# Queue (VecDeque)
dlock2 queue -t 4,8,16,32,64 --sequencial-queue-type vec-deque -d 15 --trials 3 \
    -l fc,fc-pq-b-heap,fc-ban,cc,cc-ban,mcs,mutex

# Priority Queue (BinaryHeap)
dlock2 priority-queue -t 4,8,16,32,64 -d 15 --trials 3 \
    -l fc,fc-pq-b-heap,fc-ban,cc,cc-ban,mcs,mutex

# With response time tracking
dlock2 queue -t 8,16,32 -d 5 --stat-response-time --trials 3 \
    -l fc,fc-pq-b-heap,fc-ban,mcs
dlock2 priority-queue -t 8,16,32 -d 5 --stat-response-time --trials 3 \
    -l fc,fc-pq-b-heap,fc-ban,mcs
```

### Locks

FC, FcPqBHeap, FCBan, CC, CCBan, MCS, Mutex

### Metrics

- Throughput (ops/sec) per (lock, thread_count)
- Response time percentiles (with `--stat-response-time`)

### Paper Figures

- **Supplementary:** Throughput bar chart for queue and priority queue workloads.

---

## Group 9: NUMA Stress Test

**Requires:** 2-socket NUMA machine (Cloudlab c6525-100g or equivalent).

**Hypothesis:** Delegation keeps shared data on one socket; MCS pays cross-socket
cache-line migration on every lock handoff.

**Claim validated:** Delegation's L1 locality advantage is amplified on NUMA.

### Configuration

Explicit NUMA placement: half threads on socket 0, half on socket 1.

```nu
# 2-socket spread
for t in [16 32 64] {
    dlock2 counter-proportional -t $t --cs 1000 --non-cs 0 -d 15 --trials 3 \
        -l fc,fc-pq-b-heap,mcs,mutex,shfl-lock \
        --file-name $"numa-spread-($t)"
}

# Single-socket packed (control)
for t in [8 16 32] {
    # Use numactl or core pinning to restrict to socket 0
    numactl --cpunodebind=0 --membind=0 \
        target/release/dlock -t $t d-lock2 counter-proportional \
        --cs 1000 --non-cs 0 -d 15 --trials 3 \
        -l fc,fc-pq-b-heap,mcs,mutex,shfl-lock \
        --file-name $"numa-packed-($t)"
}
```

### Locks

FC, FcPqBHeap, MCS, Mutex, ShflLock

### Metrics

- Throughput: spread vs packed ratio per lock
- `perf stat` counters: LLC-load-misses, remote-DRAM accesses

### Expected Results

- MCS: 2-3x throughput drop when spread across sockets (cross-socket migration ~100ns extra)
- FC/FC-PQ: Modest drop (only waiter inputs cross sockets, not shared data)
- The spread/packed ratio measures NUMA sensitivity — delegation should be near 1.0

### Paper Figures

- **Fig 9:** Throughput ratio (spread / packed) per lock. Bar chart. Delegation bars near 1.0, MCS/Mutex bars <<1.0.

---

## Group 10: Overhead Profiling (Hardware Counters)

**Hypothesis:** FC and FC-PQ have similar L1 cache miss rates for the shared data
structure. MCS has significantly higher LLC misses due to cross-core migration.

**Claim validated:** Hardware-level evidence for "delegation keeps shared data in
combiner's L1."

### Configuration

```bash
# FC
perf stat -e L1-dcache-load-misses,LLC-load-misses,LLC-store-misses,\
dTLB-load-misses,branch-misses,cpu-migrations,instructions,cycles \
    target/release/dlock -t 32 -d 15 d-lock2 counter-proportional \
    --cs 1000,3000 --non-cs 0 -l fc

# FC-PQ
perf stat -e L1-dcache-load-misses,LLC-load-misses,LLC-store-misses,\
dTLB-load-misses,branch-misses,cpu-migrations,instructions,cycles \
    target/release/dlock -t 32 -d 15 d-lock2 counter-proportional \
    --cs 1000,3000 --non-cs 0 -l fc-pq-b-heap

# MCS
perf stat -e L1-dcache-load-misses,LLC-load-misses,LLC-store-misses,\
dTLB-load-misses,branch-misses,cpu-migrations,instructions,cycles \
    target/release/dlock -t 32 -d 15 d-lock2 counter-proportional \
    --cs 1000,3000 --non-cs 0 -l mcs
```

### Locks

FC, FcPqBHeap, MCS (and optionally Mutex, SpinLock for contrast)

### Metrics

Per lock: L1 miss rate, LLC miss rate, IPC, branch miss rate, CPU migrations.

### Expected Results

- FC and FC-PQ: Similar L1/LLC miss rates — shared data stays in combiner's cache
- FC-PQ: Slightly more instructions (PQ maintenance) but same cache behavior
- MCS: Higher LLC misses — shared data migrates on every handoff
- SpinLock/Mutex: Highest miss rates under contention

### Paper Figures

- **Table 1:** Hardware counter comparison (FC vs FC-PQ vs MCS) at 32T.

---

## Execution Plan

### Phase A: Zero-Code Experiments (Run Immediately)

These use only the existing `counter-proportional`, `queue`, and `priority-queue`
subcommands with different configurations.

| Order | Group | Est. Time | Priority |
|-------|-------|-----------|----------|
| 1 | Group 1: CS ratio sweep | ~4h | **Critical** — central claim |
| 2 | Group 2: CS length crossover | ~1h | **Critical** — L1 locality story |
| 3 | Group 3: Non-CS sweep | ~3h | High — contention analysis |
| 4 | Group 4: Response time | ~1h | High — motivation figure |
| 5 | Group 5: Asymmetric contention | ~30m | Medium — real-world pattern |
| 6 | Group 8: Queue & PQ | ~2h | Medium — data structure generality |

Time estimates: per config = locks × threads × 15s × 3 trials. Dominated by
the number of lock variants.

### Phase B: New Application Benchmarks (Implement Then Run)

| Order | Group | Implementation Effort | Priority |
|-------|-------|-----------------------|----------|
| 7 | Group 6: Hash map | ~2 days | **Very High** — strongest application story |
| 8 | Group 7: Log buffer | ~1 day | High — write-dominated workload |

### Phase C: Hardware-Dependent

| Order | Group | Prerequisite | Priority |
|-------|-------|-------------|----------|
| 9 | Group 9: NUMA | 2-socket machine | High — hardware evidence |
| 10 | Group 10: perf stat | `perf` access | High — cache miss evidence |

---

## Paper Figure Summary

| Figure | Group | Type | Key Message |
|--------|-------|------|-------------|
| Fig 1 | G4 | CDF plot | Combiner penalty motivation |
| Fig 2 | G1 | Scatter plot | Fairness-throughput Pareto frontier |
| Fig 3 | G1 | Line plot | Throughput scales with thread count per CS ratio |
| Fig 4 | G2 | Log-log plot | Delegation crossover + constant FC-PQ overhead |
| Fig 5 | G3 | Panel plot | Contention level sensitivity |
| Fig 6 | G4 | Bar chart | p99 response time comparison |
| Fig 7 | G6 | Grouped bar | Hash map lookup p99 + throughput breakdown |
| Fig 8 | G7 | Grouped bar | Producer throughput vs drain batch size |
| Fig 9 | G9 | Bar chart | NUMA sensitivity ratio |
| Table 1 | G10 | Table | Hardware counter comparison |
