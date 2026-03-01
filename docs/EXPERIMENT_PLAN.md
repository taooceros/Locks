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
| Duration (final) | 30s | For paper-ready runs (matches TCLocks' 30s convention) |
| Duration (latency) | 5s | `--duration 5` for `--stat-response-time` (memory) |
| Output | `visualization/output` | Default |

**Note on duration:** TCLocks (OSDI'23) uses 30-second runs for all
micro-benchmarks. Our development runs use 15s for faster iteration; bump
to 30s for the final paper-ready data collection.

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
DELEGATION_UNFAIR = FC, CC, DSM, ShflLock
DELEGATION_FAIR   = FCBan, CCBan, fc-pq-b-heap
TRADITIONAL       = MCS, mutex, spin-lock, USCL
TRADITIONAL_FAIR  = CFL, Ticket, CLH
BASELINES_C       = fc-c, cc-c, shfl-lock-c
ALL               = DELEGATION_UNFAIR ∪ DELEGATION_FAIR ∪ TRADITIONAL ∪ TRADITIONAL_FAIR ∪ BASELINES_C ∪ pthread-mutex
```

CLI lock target names (for `-l` flag):
`fc, fc-ban, cc, cc-ban, dsm, fc-sl, fc-pq-b-tree, fc-pq-b-heap, mutex, spin-lock, uscl, fc-c, cc-c, mcs, shfl-lock, shfl-lock-c, cfl, ticket, clh, pthread-mutex`

### Aliases (for experiment scripts)

```nu
alias dlock2 = target/release/dlock d-lock2

let all_threads = "4,8,16,32,64,128"
let mid_threads = "8,16,32,64"

let delegation_unfair = "fc,cc,dsm"
let delegation_fair = "fc-ban,cc-ban,fc-pq-b-heap"
let traditional = "mcs,mutex,spin-lock,uscl"
let traditional_fair = "cfl,ticket,clh"
let baselines_c = "fc-c,cc-c,shfl-lock,shfl-lock-c"

let core_locks = "fc,fc-pq-b-heap,fc-ban,mcs,cfl,mutex,spin-lock,shfl-lock"
let all_locks = $"($delegation_unfair),($delegation_fair),($traditional),($traditional_fair),($baselines_c),pthread-mutex"
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

## Group 2b: Data Footprint Scaling (Counter Array)

**Hypothesis:** Delegation's throughput advantage over traditional locks grows with
the size of the shared data accessed per CS, because the combiner keeps the data
L1-hot regardless of request order. Traditional locks pay cross-core migration cost
proportional to data footprint on every handoff.

**Claim validated:** L1 locality advantage is not just about amortizing lock overhead —
it directly reduces data migration cost, and this benefit grows with working set size.

**Key finding (2026-03-01):** Sequential access hides the delegation advantage because
hardware prefetching compensates for cross-core cache migration. **Random access is
essential** to expose the true cost of data migration. Results at 32 threads:

| Array | Access | CS | FC/ShflLock | Notes |
|-------|--------|-----|-------------|-------|
| 32 KiB | sequential | 4096 | 1.03x | Prefetcher hides migration cost |
| 32 KiB | random | 4096 | 1.45x | Prefetcher defeated |
| 512 KiB | sequential | 65536 | 1.00x | Prefetcher hides migration cost |
| 512 KiB | random | 4096 | **1.98x** | L2-spill + random + frequent handoffs |

The delegation advantage is maximized when: (1) data exceeds L1 (>48 KiB on
Sapphire Rapids), (2) access pattern is random (defeats prefetching), and (3) CS is
short relative to array size (more frequent lock handoffs = more migration events for
traditional locks).

### Benchmark

The `counter-array` subcommand protects a `Vec<u64>` of configurable size (default
4096 elements = 32 KiB). Each CS invocation touches `cs_loop` elements.

New CLI flags:
- `--array-size N`: Number of u64 elements (default: 4096). Use values > 6144
  to exceed L1d cache (48 KiB on Sapphire Rapids).
- `--random-access`: Use xorshift PRNG for index generation instead of sequential.
  Defeats hardware prefetching, making cache misses after lock handoff more expensive.

### Configuration

```nu
# Sweep 1: Array size scaling with random access (delegation advantage vs footprint)
for sz in [4096 8192 16384 32768 65536 131072] {
    dlock2 counter-array -t 32 --cs 4096 --non-cs 0 --array-size $sz --random-access \
        -d 15 --trials 3 \
        -l fc,fc-pq-b-heap,fc-ban,mcs,cfl,shfl-lock \
        --file-name $"array-rand-($sz)"
}

# Sweep 2: Sequential baseline for comparison (same array sizes)
for sz in [4096 8192 16384 32768 65536 131072] {
    dlock2 counter-array -t 32 --cs 4096 --non-cs 0 --array-size $sz \
        -d 15 --trials 3 \
        -l fc,fc-pq-b-heap,fc-ban,mcs,cfl,shfl-lock \
        --file-name $"array-seq-($sz)"
}

# Thread scaling at fixed footprint (512 KiB, random, CS=4096)
for t in [4 8 16 32 64 128] {
    dlock2 counter-array -t $t --cs 4096 --non-cs 0 --array-size 65536 --random-access \
        -d 15 --trials 3 \
        --file-name $"array-rand-65536-($t)t"
}

# Direct comparison: counter-proportional vs counter-array at same CS value
for cs in [100 1000] {
    dlock2 counter-proportional -t 32 --cs $cs --non-cs 0 -d 15 --trials 3 \
        -l fc,mcs,mutex \
        --file-name $"compare-scalar-($cs)"
    dlock2 counter-array -t 32 --cs $cs --non-cs 0 --random-access -d 15 --trials 3 \
        -l fc,mcs,mutex \
        --file-name $"compare-array-($cs)"
}
```

### Locks

FC, FcPqBHeap, FCBan, MCS, CFL, ShflLock (footprint sweep)
ALL (thread scaling)

### Metrics

- Throughput vs array size (random and sequential, side-by-side)
- FC/MCS and FC/ShflLock throughput ratio at each footprint level
- Throughput vs thread count at fixed footprint

### Expected Results

- **Sequential access:** FC/ShflLock ratio stays ~1.0 regardless of array size —
  prefetching hides migration cost. This is the null result that motivates random access.
- **Random access, array <= L1 (32 KiB):** Moderate advantage (~1.4x) — cache misses
  hurt traditional locks but data still fits in L1 for the combiner.
- **Random access, array > L1 (64-1024 KiB):** Large advantage (2-3x) — after each
  handoff, traditional locks must re-fetch the entire working set from the previous
  holder's cache hierarchy. The combiner keeps data warm in its L2.
- **FC vs FC-PQ gap remains small** (~0.98x) — fairness overhead is per-combining-pass,
  not per-data-element.

### Paper Figures

- **Fig 4b:** FC/MCS throughput ratio vs array size, two lines (sequential vs random).
  Shows sequential is flat ~1x while random grows to 2-3x.
- **Fig 4c:** Throughput vs thread count at 512 KiB random. Shows delegation advantage
  grows with core count.
- **Supplementary:** counter-proportional vs counter-array comparison table.

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

## Group 11: Delegation vs ShflLock Head-to-Head

**Hypothesis:** FC-PQ achieves both higher fairness AND higher throughput than
ShflLock because delegation decouples scheduling from data migration. ShflLock
optimizes NUMA locality (queue reordering by node) but still migrates shared
data on every lock handoff and has no usage tracking for fairness.

**Claim validated:** Delegation's L1 locality advantage applies even against
NUMA-aware traditional locks. ShflLock's queue shuffling reduces inter-socket
transfers but cannot match keeping data permanently in one L1.

### Dimension 1: Fairness Under Heterogeneous CS

ShflLock has no usage tracking — long-CS threads accumulate proportionally more
hold-time, degrading JFI. FC-PQ's priority queue rebalances by usage.

```nu
let locks = "fc,fc-pq-b-heap,shfl-lock,mcs"
for cs in ["1000,1000" "1000,3000" "1000,10000" "1000,30000" "100,10000"] {
    dlock2 counter-proportional -t 32 --cs $cs --non-cs 0 -d 15 --trials 3 \
        -l $locks --file-name $"shfl-fairness-($cs)"
}
```

#### Expected Results

- **1:1 (control):** All locks achieve JFI ~1.0. ShflLock and MCS competitive on throughput.
- **1:3 to 1:10:** ShflLock JFI degrades (similar trajectory to MCS). FC-PQ stays >0.99.
- **1:30 to 1:100:** ShflLock JFI drops below 0.7. FC-PQ still >0.99.
- FC throughput ≈ FC-PQ throughput >> ShflLock ≈ MCS (delegation advantage).

### Dimension 2: Throughput via L1 Locality (Data Footprint Scaling)

ShflLock = traditional lock: shared data migrates between cores on every handoff.
Even with NUMA-aware queue ordering, the data still moves core-to-core within a
socket. FC-PQ = delegation: shared data stays permanently in combiner's L1/L2.

**Critical: use `--random-access` to defeat hardware prefetching.** Sequential access
lets the prefetcher hide migration cost, producing a misleading FC/ShflLock ≈ 1.0x
result. Random access exposes the true cache migration penalty. (Validated 2026-03-01:
sequential gives 1.03x, random gives 1.45-1.98x at 32T.)

**Use `--array-size` to exceed L1 cache (48 KiB on Sapphire Rapids).** The default
4096-element array (32 KiB) fits entirely in L1, so even after a lock handoff the
new holder warms its cache quickly. With array-size=65536 (512 KiB), the working
set spills to L2, and each handoff forces expensive re-fetching.

```nu
# Array size sweep with random access (key experiment)
for sz in [4096 8192 16384 32768 65536 131072] {
    dlock2 counter-array -t 32 --cs 4096 --non-cs 0 \
        --array-size $sz --random-access -d 15 --trials 3 \
        -l $locks --file-name $"shfl-footprint-rand-($sz)"
}

# Sequential baseline for comparison
for sz in [4096 8192 16384 32768 65536 131072] {
    dlock2 counter-array -t 32 --cs 4096 --non-cs 0 \
        --array-size $sz -d 15 --trials 3 \
        -l $locks --file-name $"shfl-footprint-seq-($sz)"
}

# Thread scaling at fixed footprint (512 KiB, random, CS=4096)
for t in [4 8 16 32 64 128] {
    dlock2 counter-array -t $t --cs 4096 --non-cs 0 \
        --array-size 65536 --random-access -d 15 --trials 3 \
        -l $locks --file-name $"shfl-array-rand-65536-($t)t"
}
```

#### Expected Results

- **Sequential access (any size):** FC/ShflLock ≈ 1.0x — prefetcher hides migration.
  This is the **null result** that motivates random access.
- **Random, array ≤ L1 (32 KiB):** FC/ShflLock ≈ 1.4x — cache misses hurt but data
  still fits in L1 for the combiner.
- **Random, array > L1 (64-1024 KiB):** FC/ShflLock ≈ 2-3x — each handoff forces
  traditional locks to re-fetch the working set from the previous holder's L2/L3.
  The combiner keeps data warm in its own cache hierarchy.
- FC/FC-PQ ratio should be ~0.98-1.0 across all footprints (fairness overhead is
  per-pass, not per-element).
- Thread scaling: delegation advantage grows with thread count (more cores competing
  to cache shared data).

### Dimension 3: Tail Latency Under Asymmetric Contention

Half the threads are "hot" (non-CS=0, always contending) and half are "cold"
(non-CS=10000, arriving infrequently). ShflLock's queue reordering favors hot
threads (more frequent arrivals). FC-PQ compensates via usage-based priority.

```nu
# Asymmetric contention: hot vs cold threads
dlock2 counter-proportional -t 8,16,32,64 --cs 1000,1000 --non-cs 0,10000 \
    -d 5 --stat-response-time --trials 3 \
    -l $locks --file-name "shfl-asymmetric"

# Worst case: heterogeneous CS + asymmetric arrival
dlock2 counter-proportional -t 32 --cs 1000,10000 --non-cs 0,10000 \
    -d 5 --stat-response-time --trials 3 \
    -l $locks --file-name "shfl-worst-case"
```

#### Expected Results

- **Asymmetric contention:** ShflLock p99 for cold threads >> FC-PQ p99 for cold threads.
  FC-PQ's priority queue boosts underserved cold threads.
- **Worst case (heterogeneous CS + asymmetric arrival):** Largest JFI gap between
  ShflLock and FC-PQ. ShflLock JFI < 0.6, FC-PQ JFI > 0.95.
- Per-thread normalized share: FC-PQ bars near 1.0, ShflLock bars heavily skewed
  toward hot + long-CS threads.

### Paper Figures

- **Fig 10a:** JFI vs CS ratio for FC, FC-PQ, ShflLock, MCS. Shows ShflLock and MCS
  degrade together while FC-PQ stays flat.
- **Fig 10b:** FC-PQ/ShflLock throughput ratio vs array size, two lines (sequential
  vs random). Sequential flat ~1x; random grows from ~1.4x at 32 KiB to 2-3x at
  512 KiB+. Demonstrates that prefetching masks the migration cost in sequential
  benchmarks — random access is needed to expose delegation's true locality advantage.
- **Fig 10c:** Per-thread normalized share bar chart under worst-case workload.
  FC-PQ uniform, ShflLock heavily skewed.

---

## Group 12: Factor Analysis (FC-PQ Overhead Decomposition)

**Inspired by:** TCLocks (OSDI'23) Fig 5g, which decomposes TCLock's improvement
into Base → +NUMA → +Prefetch → +WWJump. We apply the same technique to
decompose FC-PQ's overhead relative to FC.

**Hypothesis:** Most of the FC→FC-PQ throughput gap comes from priority queue
maintenance (O(log N) per CS), not from fairness enforcement per se. Prefetching
and starvation bounds add negligible overhead.

**Claim validated:** The cost of fairness in delegation is dominated by a small,
well-characterized scheduling overhead, not by data movement or synchronization.

### Configuration

We need FC-PQ builds with individual features toggled. Since these are compile-time
features, this requires conditional compilation or separate benchmark functions.

**Approach:** Use the existing FC (unfair) and FC-PQ (all optimizations) as the
two endpoints. Also benchmark FC-Ban as a different fairness strategy. The
decomposition is:

```
FC (base)           → Pure unfair delegation throughput
FC-Ban              → Banning fairness overhead (non-work-conserving)
FC-PQ (no prefetch) → PQ scheduling overhead without prefetch optimization
FC-PQ (full)        → Full FC-PQ with prefetch + starvation bound
```

```nu
let locks = "fc,fc-ban,fc-pq-b-heap"
# Fixed workload: 32T, 3:1 CS ratio, high contention
dlock2 counter-proportional -t 32 --cs 1000,3000 --non-cs 0 -d 15 --trials 3 \
    -l $locks --file-name "factor-analysis-3to1"
# Also at 1:10 and uniform
dlock2 counter-proportional -t 32 --cs 1000,10000 --non-cs 0 -d 15 --trials 3 \
    -l $locks --file-name "factor-analysis-1to10"
dlock2 counter-proportional -t 32 --cs 1000 --non-cs 0 -d 15 --trials 3 \
    -l $locks --file-name "factor-analysis-uniform"
# Thread scaling at fixed ratio
for tc in [4 8 16 32 64 128] {
    dlock2 counter-proportional -t $tc --cs 1000,3000 --non-cs 0 -d 15 --trials 3 \
        -l $locks --file-name $"factor-analysis-($tc)t"
}
```

### Metrics

- Throughput: FC, FC-Ban, FC-PQ at each configuration
- Throughput delta: (FC - FC-PQ) / FC = fairness overhead percentage
- JFI at each step: FC (low), FC-Ban (high), FC-PQ (high)
- Throughput-per-JFI-point: how much throughput does each JFI improvement cost?

### Expected Results

- FC→FC-PQ throughput delta: 5-15% (PQ maintenance over L1-hot data)
- FC→FC-Ban throughput delta: 10-25% (non-work-conserving, banned threads idle)
- FC-PQ JFI ≥ FC-Ban JFI (work-conserving achieves similar fairness)
- At uniform CS (1:1): FC ≈ FC-PQ ≈ FC-Ban (no fairness needed, all equivalent)
- Overhead scales sub-linearly with thread count (PQ is O(log N), not O(N))

### Paper Figures

- **Fig 11:** Factor analysis bar chart at 32T. Grouped bars for FC, FC-Ban, FC-PQ
  showing throughput and JFI side by side. Annotated arrows showing % overhead.
- **Table 2:** FC-PQ overhead percentage vs FC at each thread count and CS ratio.

---

## Execution Plan

### Phase A: Zero-Code Experiments (Run Immediately)

These use existing subcommands (`counter-proportional`, `counter-array`, `queue`,
`priority-queue`) with different configurations.

| Order | Group | Est. Time | Priority |
|-------|-------|-----------|----------|
| 1 | Group 1: CS ratio sweep | ~4h | **Critical** — central claim |
| 2 | Group 2: CS length crossover | ~1h | **Critical** — L1 locality story |
| 2b | Group 2b: Data footprint scaling | ~1.5h | **Critical** — L1 data migration evidence |
| 3 | Group 3: Non-CS sweep | ~3h | High — contention analysis |
| 4 | Group 4: Response time | ~1h | High — motivation figure |
| 5 | Group 5: Asymmetric contention | ~30m | Medium — real-world pattern |
| 6 | Group 8: Queue & PQ | ~2h | Medium — data structure generality |
| 11 | Group 11: Delegation vs ShflLock | ~2h | **High** — ShflLock comparison |
| 12 | Group 12: Factor analysis | ~30m | **High** — overhead decomposition |

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
| Fig 4a | G2 | Log-log plot | Delegation crossover + constant FC-PQ overhead |
| Fig 4b | G2b | Bar/line chart | FC/MCS ratio grows with data footprint |
| Fig 5 | G3 | Panel plot | Contention level sensitivity |
| Fig 6 | G4 | Bar chart | p99 response time comparison |
| Fig 7 | G6 | Grouped bar | Hash map lookup p99 + throughput breakdown |
| Fig 8 | G7 | Grouped bar | Producer throughput vs drain batch size |
| Fig 9 | G9 | Bar chart | NUMA sensitivity ratio |
| Fig 10a | G11 | Line plot | JFI vs CS ratio: ShflLock degrades like MCS, FC-PQ flat |
| Fig 10b | G11 | Line/bar chart | FC-PQ/ShflLock throughput ratio vs data footprint |
| Fig 10c | G11 | Bar chart | Per-thread normalized share under worst-case workload |
| Fig 11 | G12 | Grouped bar | Factor analysis: FC vs FC-Ban vs FC-PQ overhead |
| Table 1 | G10 | Table | Hardware counter comparison |
| Table 2 | G12 | Table | FC-PQ overhead % vs FC at each thread count and ratio |
