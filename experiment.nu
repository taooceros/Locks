#!/usr/bin/env nu
# Experiment runner for delegation lock benchmarks.
#
# Usage (run individual groups via copy-paste or source this file):
#   nu experiment.nu              # Run full micro-benchmark suite
#
# Or source and call individual functions:
#   source experiment.nu
#   smoke                         # Quick smoke test (~2 min)
#   group1                        # CS ratio sweep
#   group2                        # CS length crossover
#   group2b                       # Data footprint scaling
#   group3                        # Non-CS sweep
#   group4                        # Response time distributions
#   group5                        # Asymmetric contention
#   group8                        # Queue & priority queue
#   group_hashmap                 # Concurrent hash map (heterogeneous CS)
#   group_benefits                # Benefits of delegation with fairness (Pareto scatter)
#   group_tradeoff                # Phase 3: fairness-performance tradeoff
#   group_combiner                # Phase 2: combiner response-time study
#   group_perf                    # Phase 3: perf stat cache miss validation
#   group_delegation_vs_shfl      # Delegation vs ShflLock head-to-head
#   group_factor_analysis         # Factor analysis: FC-PQ overhead decomposition
#
# See docs/EXPERIMENT_PLAN.md for detailed experiment specs.

cargo build --release

# ─── Common parameters ───────────────────────────────────────────────

let all_threads = "4,8,16,32,64,128"
let mid_threads = "8,16,32,64"
let few_threads = "8,16,32"

let delegation_unfair = "fc,cc,dsm"
let delegation_fair = "fc-ban,cc-ban,fc-pq-b-heap"
let traditional = "mcs,mutex,spin-lock,uscl"
let traditional_fair = "cfl,ticket,clh"
let traditional_all = $"($traditional),($traditional_fair),pthread-mutex"
let baselines_c = "fc-c,cc-c,shfl-lock,shfl-lock-c"

let core_locks = "fc,fc-pq-b-heap,fc-ban,mcs,cfl,mutex,spin-lock,shfl-lock"
let all_locks = $"($delegation_unfair),($delegation_fair),($traditional_all),($baselines_c)"

# Phase 3: tradeoff scatter plot locks
let tradeoff_locks = "fc,fc-pq-b-heap,fc-ban,mcs,cfl,spin-lock,mutex,ticket,clh,shfl-lock"

# Benefits demonstration: delegation vs traditional, unfair vs fair
let benefits_locks = "fc,fc-pq-b-heap,fc-ban,mcs,cfl,mutex,spin-lock,shfl-lock"

let full_duration = 15
let latency_duration = 5
let trials = 3

let dlock2 = "target/release/dlock"

# ─── Smoke test (~2 min) ────────────────────────────────────────────
# Quick sanity check: 2 configs, short duration, few threads, few locks.

def smoke [] {
    print "=== Smoke test ==="
    run-external $dlock2 d-lock2 -t 4,16 -l fc,fc-pq-b-heap,mcs,cfl counter-proportional --cs 1000,3000 --non-cs 0 -d 2 --trials 1 --file-name smoke-test
    run-external $dlock2 d-lock2 -t 4,16 -l fc,mcs counter-array --cs 100 --non-cs 0 -d 2 --trials 1 --file-name smoke-array
    run-external $dlock2 d-lock2 -t 4,16 -l fc,mcs queue -d 2 --trials 1
    run-external $dlock2 d-lock2 -t 4 -l fc,fc-pq-b-heap,mcs hash-map --scan-threads 1 --scan-size 100 -d 2 --trials 1 --file-name smoke-hashmap
    print "=== Smoke test done ==="
}

# ─── Group 1: CS Ratio Sweep ────────────────────────────────────────
# Validates FC-PQ maintains JFI near 1.0 regardless of CS ratio.
# Includes all locks (delegation + traditional + fair baselines).

def group1 [] {
    print "=== Group 1: CS Ratio Sweep ==="
    for cs in ["1000,1000" "1000,3000" "1000,10000" "100,3000" "100,10000"] {
        run-external $dlock2 d-lock2 -t $all_threads -l $all_locks counter-proportional --cs $cs --non-cs 0 -d $full_duration --trials $trials
    }
}

# ─── Group 2: CS Length Scalability Crossover ────────────────────────
# Shows delegation advantage grows with CS length (L1 locality).
# Includes CFL for fairness-cost comparison.

def group2 [] {
    print "=== Group 2: CS Length Scalability Crossover ==="
    for cs in [1 10 100 1000 5000 10000 50000] {
        run-external $dlock2 d-lock2 -t 32 -l $core_locks counter-proportional --cs ($cs | into string) --non-cs 0 -d $full_duration --trials $trials --file-name $"cs-crossover-($cs)"
    }
}

# ─── Group 2b: Data Footprint Scaling (Counter Array) ────────────────
# Shows data migration cost grows with working set size.
# IMPORTANT: Use --random-access to defeat hardware prefetching.
# Sequential access lets the prefetcher hide migration cost (FC/ShflLock ≈ 1.0x).
# Random access exposes the true cache migration penalty (FC/ShflLock ≈ 2x at 512 KiB).

def group2b [] {
    print "=== Group 2b: Data Footprint Scaling ==="

    # Array size sweep with random access (key experiment for delegation advantage)
    for sz in [4096 8192 16384 32768 65536 131072] {
        run-external $dlock2 d-lock2 -t 32 -l $core_locks counter-array --cs 4096 --non-cs 0 --array-size ($sz | into string) --random-access -d $full_duration --trials $trials --file-name $"array-rand-($sz)"
    }

    # Sequential baseline for comparison
    for sz in [4096 8192 16384 32768 65536 131072] {
        run-external $dlock2 d-lock2 -t 32 -l $core_locks counter-array --cs 4096 --non-cs 0 --array-size ($sz | into string) -d $full_duration --trials $trials --file-name $"array-seq-($sz)"
    }

    # Thread scaling at fixed footprint (512 KiB random, CS=4096)
    run-external $dlock2 d-lock2 -t $all_threads -l $core_locks counter-array --cs 4096 --non-cs 0 --array-size 65536 --random-access -d $full_duration --trials $trials --file-name array-rand-65536

    for cs in [100 1000] {
        run-external $dlock2 d-lock2 -t 32 -l fc,mcs,mutex counter-proportional --cs ($cs | into string) --non-cs 0 -d $full_duration --trials $trials --file-name $"compare-scalar-($cs)"
        run-external $dlock2 d-lock2 -t 32 -l fc,mcs,mutex counter-array --cs ($cs | into string) --non-cs 0 --random-access -d $full_duration --trials $trials --file-name $"compare-array-($cs)"
    }
}

# ─── Group 3: Non-CS Sweep (Contention Levels) ──────────────────────
# Delegation advantage largest under high contention.
# Includes CFL for fairness comparison across contention levels.

def group3 [] {
    print "=== Group 3: Non-CS Sweep ==="
    for non_cs in [0 10 100 1000 10000 100000] {
        run-external $dlock2 d-lock2 -t $all_threads -l fc,fc-pq-b-heap,fc-ban,mcs,cfl,mutex,uscl,shfl-lock counter-proportional --cs 1000,3000 --non-cs ($non_cs | into string) -d $full_duration --trials $trials
    }
}

# ─── Group 4: Response Time Distributions ────────────────────────────
# Combiner vs waiter latency CDFs. Requires --stat-response-time.
# Includes CFL for response time comparison.

def group4 [] {
    print "=== Group 4: Response Time Distributions ==="

    run-external $dlock2 d-lock2 -t $few_threads -l fc,fc-pq-b-heap,fc-ban,cc,cc-ban,mcs,cfl,shfl-lock --stat-response-time counter-proportional --cs 1 --non-cs 0 -d $latency_duration --trials $trials --file-name latency-cs1

    run-external $dlock2 d-lock2 -t $few_threads -l fc,fc-pq-b-heap,fc-ban,cc,cc-ban,mcs,cfl,shfl-lock --stat-response-time counter-proportional --cs 1000,3000 --non-cs 0 -d $latency_duration --trials $trials --file-name latency-cs1000-3000
}

# ─── Group 5: Asymmetric Contention ─────────────────────────────────
# Hot (non-CS=0) vs cold (non-CS=10000) threads.
# Includes CFL for fairness comparison under asymmetric load.

def group5 [] {
    print "=== Group 5: Asymmetric Contention ==="
    run-external $dlock2 d-lock2 -t $mid_threads -l fc,fc-pq-b-heap,fc-ban,mcs,cfl,uscl,shfl-lock counter-proportional --cs 1000,1000 --non-cs 0,10000 -d $full_duration --trials $trials --file-name asymmetric-contention
}

# ─── Group 8: Queue & Priority Queue ────────────────────────────────
# Delegation advantage on sequential data structures.

def group8 [] {
    print "=== Group 8: Queue & Priority Queue ==="

    let q_locks = "fc,fc-pq-b-heap,fc-ban,cc,cc-ban,mcs,mutex"

    # Queue (LinkedList)
    run-external $dlock2 d-lock2 -t "4,8,16,32,64" -l $q_locks queue -d $full_duration --trials $trials

    # Queue (VecDeque)
    run-external $dlock2 d-lock2 -t "4,8,16,32,64" -l $q_locks queue --sequencial-queue-type vec-deque -d $full_duration --trials $trials

    # Priority Queue (BinaryHeap)
    run-external $dlock2 d-lock2 -t "4,8,16,32,64" -l $q_locks priority-queue -d $full_duration --trials $trials

    # With response time
    run-external $dlock2 d-lock2 -t $few_threads -l fc,fc-pq-b-heap,fc-ban,mcs --stat-response-time queue -d $latency_duration --trials $trials
    run-external $dlock2 d-lock2 -t $few_threads -l fc,fc-pq-b-heap,fc-ban,mcs --stat-response-time priority-queue -d $latency_duration --trials $trials
}

# ─── Fetch and Multiply ─────────────────────────────────────────────

def fetch_and_multiply [] {
    print "=== Fetch and Multiply ==="
    run-external $dlock2 d-lock2 -t $all_threads fetch-and-multiply -d $full_duration --trials $trials
    run-external $dlock2 d-lock2 -t $few_threads --stat-response-time fetch-and-multiply -d $latency_duration --trials $trials
}

# ─── Benefits of Delegation with Fairness ────────────────────────────
# KEY EXPERIMENT: Produces the Pareto scatter plot (Fig 2 in paper).
# Shows FC-PQ uniquely occupies high-throughput + high-fairness corner.
#   FC→FC-PQ: small throughput cost, large fairness gain
#   MCS→CFL: large throughput cost, large fairness gain

def group_benefits [] {
    print "=== Benefits of Delegation with Fairness ==="

    # CS ratio sweep at 32T: the scatter plot data
    for cs in ["1000,1000" "1000,3000" "1000,10000" "100,3000" "100,10000"] {
        run-external $dlock2 d-lock2 -t 32 -l $benefits_locks counter-proportional --cs $cs --non-cs 0 -d $full_duration --trials $trials --file-name $"benefits-32t-cs-($cs)"
    }

    # Thread scaling at 1:10 ratio (moderate heterogeneity)
    run-external $dlock2 d-lock2 -t $all_threads -l $benefits_locks counter-proportional --cs 1000,10000 --non-cs 0 -d $full_duration --trials $trials --file-name "benefits-1to10"

    # Data footprint: counter-array at 32T with random access (L1 locality persists with fairness)
    for sz in [4096 65536] {
        run-external $dlock2 d-lock2 -t 32 -l $benefits_locks counter-array --cs 4096 --non-cs 0 --array-size ($sz | into string) --random-access -d $full_duration --trials $trials --file-name $"benefits-array-rand-($sz)"
    }
}

# ─── Phase 3: Fairness-Performance Tradeoff ──────────────────────────
# Central claim: FC→FC-PQ throughput gap << MCS→CFL throughput gap.
# Produces scatter data for the Pareto frontier plot.

def group_tradeoff [] {
    print "=== Phase 3: Fairness-Performance Tradeoff ==="

    # Primary: 3:1 CS ratio at multiple thread counts
    for tc in [8 16 32 64] {
        run-external $dlock2 d-lock2 -t ($tc | into string) -l $tradeoff_locks counter-proportional --cs 1000,3000 --non-cs 0 -d $full_duration --trials $trials --file-name $"tradeoff-3to1-($tc)t"
    }

    # Wider ratios at 32T to stress fairness
    for cs in ["1000,10000" "100,10000"] {
        run-external $dlock2 d-lock2 -t 32 -l $tradeoff_locks counter-proportional --cs $cs --non-cs 0 -d $full_duration --trials $trials --file-name $"tradeoff-wide-($cs)"
    }
}

# ─── Phase 2: Combiner Response-Time Study ───────────────────────────
# Characterizes combiner time distribution, combiner penalty scaling,
# and split combiner vs waiter CDFs across thread counts.

def group_combiner [] {
    print "=== Phase 2: Combiner Response-Time Study ==="
    let locks = "fc,fc-pq-b-heap,fc-ban,cc,cc-ban,mcs,cfl,shfl-lock"

    # Combiner time distribution across thread counts
    for tc in [4 8 16 32 64] {
        run-external $dlock2 d-lock2 -t ($tc | into string) -l $locks --stat-response-time counter-proportional --cs 1000,3000 --non-cs 0 -d $latency_duration --trials $trials --file-name $"combiner-study-($tc)t"
    }

    # Extreme heterogeneity: combiner penalty most pronounced
    run-external $dlock2 d-lock2 -t $few_threads -l $locks --stat-response-time counter-proportional --cs 100,10000 --non-cs 0 -d $latency_duration --trials $trials --file-name "combiner-study-1to100"
}

# ─── Phase 3: perf stat Cache Miss Validation ────────────────────────
# Validates FC and FC-PQ have similar L1 miss rates.
# CFL should show higher LLC misses due to cross-core migration.

def group_perf [] {
    print "=== Phase 3: perf stat Cache Miss Validation ==="
    mkdir profiles

    let perf_events = "L1-dcache-load-misses,L1-dcache-loads,LLC-load-misses,LLC-loads,LLC-store-misses,dTLB-load-misses,cpu-migrations,instructions,cycles,branch-misses"

    for lock in [fc fc-pq-b-heap fc-ban mcs cfl mutex spin-lock ticket clh shfl-lock] {
        print $"Profiling ($lock)..."
        (perf stat -e $perf_events
            $dlock2 d-lock2 -t 32 -l $lock counter-proportional
            --cs 1000,3000 --non-cs 0 -d 15
            out> $"profiles/tradeoff-($lock).txt"
            err> $"profiles/tradeoff-($lock).stats")
    }
}

# ─── Delegation vs ShflLock Head-to-Head ─────────────────────────────
# Directly demonstrates FC-PQ advantages over ShflLock across 3 dimensions:
#   1. Fairness under heterogeneous CS (ShflLock has no usage tracking)
#   2. Throughput via L1 locality (ShflLock migrates shared data on every handoff)
#   3. Tail latency under asymmetric contention
#
# Paper story: ShflLock optimizes NUMA locality but ignores fairness.
# FC-PQ achieves both fairness AND throughput because delegation decouples
# scheduling from data migration.

def group_delegation_vs_shfl [] {
    print "=== Delegation vs ShflLock Head-to-Head ==="
    let locks = "fc,fc-pq-b-heap,shfl-lock,mcs"

    # ── Dimension 1: Fairness under heterogeneous CS ──
    # ShflLock has no usage tracking → long-CS threads monopolize
    # FC-PQ rebalances via priority queue → JFI stays near 1.0
    # Sweep CS ratio from 1:1 (control) to 1:100 (extreme)
    print "--- Dimension 1: Fairness under heterogeneous CS ---"
    for cs in ["1000,1000" "1000,3000" "1000,10000" "1000,30000" "100,10000"] {
        run-external $dlock2 d-lock2 -t 32 -l $locks counter-proportional --cs $cs --non-cs 0 -d $full_duration --trials $trials --file-name $"shfl-fairness-($cs)"
    }

    # ── Dimension 2: Throughput via L1 locality ──
    # ShflLock = traditional lock: shared data migrates between cores on every handoff
    # FC-PQ = delegation: shared data stays in combiner's L1/L2
    # IMPORTANT: Use --random-access to defeat hardware prefetching.
    # Sequential gives FC/ShflLock ≈ 1.0x; random gives 1.4-2.0x.
    # Use --array-size > 6144 to exceed L1 (48 KiB on Sapphire Rapids).
    print "--- Dimension 2: L1 locality (array size + random access) ---"
    for sz in [4096 8192 16384 32768 65536 131072] {
        run-external $dlock2 d-lock2 -t 32 -l $locks counter-array --cs 4096 --non-cs 0 --array-size ($sz | into string) --random-access -d $full_duration --trials $trials --file-name $"shfl-footprint-rand-($sz)"
    }

    # Sequential baseline for comparison (shows prefetcher hides migration)
    for sz in [4096 8192 16384 32768 65536 131072] {
        run-external $dlock2 d-lock2 -t 32 -l $locks counter-array --cs 4096 --non-cs 0 --array-size ($sz | into string) -d $full_duration --trials $trials --file-name $"shfl-footprint-seq-($sz)"
    }

    # Thread scaling at fixed footprint (512 KiB, random, CS=4096)
    for tc in [4 8 16 32 64 128] {
        run-external $dlock2 d-lock2 -t ($tc | into string) -l $locks counter-array --cs 4096 --non-cs 0 --array-size 65536 --random-access -d $full_duration --trials $trials --file-name $"shfl-array-rand-65536-($tc)t"
    }

    # ── Dimension 3: Tail latency under asymmetric contention ──
    # Hot threads (non-CS=0) vs cold threads (non-CS=10000)
    # ShflLock: hot threads dominate, cold threads starved
    # FC-PQ: usage priority compensates for arrival rate differences
    print "--- Dimension 3: Tail latency (asymmetric contention) ---"
    run-external $dlock2 d-lock2 -t 8,16,32,64 -l $locks --stat-response-time counter-proportional --cs 1000,1000 --non-cs 0,10000 -d $latency_duration --trials $trials --file-name "shfl-asymmetric"

    # Heterogeneous CS + asymmetric arrival = worst case for ShflLock
    run-external $dlock2 d-lock2 -t 32 -l $locks --stat-response-time counter-proportional --cs 1000,10000 --non-cs 0,10000 -d $latency_duration --trials $trials --file-name "shfl-worst-case"
}

# ─── Group 6: Concurrent Hash Map ─────────────────────────────────
# Delegation advantage on realistic data structure with heterogeneous CS.
# Hypothesis: FC-PQ bounds lookup p99 even with long-CS scan threads.

def group_hashmap [] {
    print "=== Group 6: Concurrent Hash Map ==="
    let locks = "fc,fc-pq-b-heap,fc-ban,mcs,cfl,mutex,shfl-lock"

    # Scan size sweep at 32T (primary experiment)
    for ss in [100 500 1000] {
        run-external $dlock2 d-lock2 -t 32 -l $locks --stat-response-time hash-map --scan-threads 2 --scan-size ($ss | into string) -d $full_duration --trials $trials --file-name $"hashmap-scan-($ss)"
    }

    # Thread scaling at scan-size=500
    for tc in [8 16 32 64] {
        run-external $dlock2 d-lock2 -t ($tc | into string) -l $locks --stat-response-time hash-map --scan-threads 2 --scan-size 500 -d $latency_duration --trials $trials --file-name $"hashmap-($tc)t"
    }

    # Scan thread count sweep at 32T
    for st in [0 1 2 4] {
        run-external $dlock2 d-lock2 -t 32 -l $locks hash-map --scan-threads ($st | into string) --scan-size 500 -d $full_duration --trials $trials --file-name $"hashmap-scanners-($st)"
    }
}

# ─── Factor Analysis (FC-PQ Overhead Decomposition) ──────────────────
# Inspired by TCLocks (OSDI'23) Fig 5g: decompose throughput gap into
# additive components. Compare FC (base), FC-Ban (banning fairness),
# and FC-PQ (PQ scheduling fairness) at fixed workloads.
# Produces the "cost of fairness" table and bar chart.

def group_factor_analysis [] {
    print "=== Factor Analysis: FC-PQ Overhead Decomposition ==="
    let locks = "fc,fc-ban,fc-pq-b-heap"

    # Fixed workloads at 32T: uniform, 3:1, 1:10
    run-external $dlock2 d-lock2 -t 32 -l $locks counter-proportional --cs 1000 --non-cs 0 -d $full_duration --trials $trials --file-name "factor-analysis-uniform"
    run-external $dlock2 d-lock2 -t 32 -l $locks counter-proportional --cs 1000,3000 --non-cs 0 -d $full_duration --trials $trials --file-name "factor-analysis-3to1"
    run-external $dlock2 d-lock2 -t 32 -l $locks counter-proportional --cs 1000,10000 --non-cs 0 -d $full_duration --trials $trials --file-name "factor-analysis-1to10"

    # Thread scaling at fixed 3:1 ratio
    for tc in [4 8 16 32 64 128] {
        run-external $dlock2 d-lock2 -t ($tc | into string) -l $locks counter-proportional --cs 1000,3000 --non-cs 0 -d $full_duration --trials $trials --file-name $"factor-analysis-($tc)t"
    }
}

# ─── Full suite ──────────────────────────────────────────────────────

print "=== Running full micro-benchmark suite ==="
print $"Machine: (sys host | get hostname)"
print $"Threads: ($all_threads)"
print $"Duration: ($full_duration)s throughput, ($latency_duration)s latency"
print $"Trials: ($trials)"
print ""

group1
group2
group2b
group3
group4
group5
group8
group_hashmap
fetch_and_multiply
group_benefits
group_tradeoff
group_combiner
group_delegation_vs_shfl
group_factor_analysis

print "=== Full suite complete ==="
