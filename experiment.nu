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
let baselines_c = "fc-c,cc-c,shfl-lock,shfl-lock-c"
let core_locks = "fc,fc-pq-b-heap,fc-ban,mcs,mutex,spin-lock"
let all_locks = $"($delegation_unfair),($delegation_fair),($traditional),($baselines_c)"

let full_duration = 15
let latency_duration = 5
let trials = 3

let dlock2 = "target/release/dlock"

# ─── Smoke test (~2 min) ────────────────────────────────────────────
# Quick sanity check: 2 configs, short duration, few threads, few locks.

def smoke [] {
    print "=== Smoke test ==="
    run-external $dlock2 d-lock2 -t 4,16 -l fc,fc-pq-b-heap,mcs counter-proportional --cs 1000,3000 --non-cs 0 -d 2 --trials 1 --file-name smoke-test
    run-external $dlock2 d-lock2 -t 4,16 -l fc,mcs counter-array --cs 100 --non-cs 0 -d 2 --trials 1 --file-name smoke-array
    run-external $dlock2 d-lock2 -t 4,16 -l fc,mcs queue -d 2 --trials 1
    print "=== Smoke test done ==="
}

# ─── Group 1: CS Ratio Sweep ────────────────────────────────────────
# Validates FC-PQ maintains JFI near 1.0 regardless of CS ratio.

def group1 [] {
    print "=== Group 1: CS Ratio Sweep ==="
    for cs in ["1000,1000" "1000,3000" "1000,10000" "100,3000" "100,10000"] {
        run-external $dlock2 d-lock2 -t $all_threads counter-proportional --cs $cs --non-cs 0 -d $full_duration --trials $trials
    }
}

# ─── Group 2: CS Length Scalability Crossover ────────────────────────
# Shows delegation advantage grows with CS length (L1 locality).

def group2 [] {
    print "=== Group 2: CS Length Scalability Crossover ==="
    for cs in [1 10 100 1000 5000 10000 50000] {
        run-external $dlock2 d-lock2 -t 32 -l $core_locks counter-proportional --cs ($cs | into string) --non-cs 0 -d $full_duration --trials $trials --file-name $"cs-crossover-($cs)"
    }
}

# ─── Group 2b: Data Footprint Scaling (Counter Array) ────────────────
# Shows data migration cost grows with working set size.

def group2b [] {
    print "=== Group 2b: Data Footprint Scaling ==="

    for cs in [1 10 100 500 1000 2000 4096] {
        run-external $dlock2 d-lock2 -t 32 -l $core_locks counter-array --cs ($cs | into string) --non-cs 0 -d $full_duration --trials $trials --file-name $"array-footprint-($cs)"
    }

    run-external $dlock2 d-lock2 -t $all_threads counter-array --cs 100 --non-cs 0 -d $full_duration --trials $trials --file-name array-cs100

    for cs in [100 1000] {
        run-external $dlock2 d-lock2 -t 32 -l fc,mcs,mutex counter-proportional --cs ($cs | into string) --non-cs 0 -d $full_duration --trials $trials --file-name $"compare-scalar-($cs)"
        run-external $dlock2 d-lock2 -t 32 -l fc,mcs,mutex counter-array --cs ($cs | into string) --non-cs 0 -d $full_duration --trials $trials --file-name $"compare-array-($cs)"
    }
}

# ─── Group 3: Non-CS Sweep (Contention Levels) ──────────────────────
# Delegation advantage largest under high contention.

def group3 [] {
    print "=== Group 3: Non-CS Sweep ==="
    for non_cs in [0 10 100 1000 10000 100000] {
        run-external $dlock2 d-lock2 -t $all_threads -l fc,fc-pq-b-heap,fc-ban,mcs,mutex,uscl counter-proportional --cs 1000,3000 --non-cs ($non_cs | into string) -d $full_duration --trials $trials
    }
}

# ─── Group 4: Response Time Distributions ────────────────────────────
# Combiner vs waiter latency CDFs. Requires --stat-response-time.

def group4 [] {
    print "=== Group 4: Response Time Distributions ==="

    run-external $dlock2 d-lock2 -t $few_threads -l fc,fc-pq-b-heap,fc-ban,cc,cc-ban,mcs --stat-response-time counter-proportional --cs 1 --non-cs 0 -d $latency_duration --trials $trials --file-name latency-cs1

    run-external $dlock2 d-lock2 -t $few_threads -l fc,fc-pq-b-heap,fc-ban,cc,cc-ban,mcs --stat-response-time counter-proportional --cs 1000,3000 --non-cs 0 -d $latency_duration --trials $trials --file-name latency-cs1000-3000
}

# ─── Group 5: Asymmetric Contention ─────────────────────────────────
# Hot (non-CS=0) vs cold (non-CS=10000) threads.

def group5 [] {
    print "=== Group 5: Asymmetric Contention ==="
    run-external $dlock2 d-lock2 -t $mid_threads -l fc,fc-pq-b-heap,fc-ban,mcs,uscl counter-proportional --cs 1000,1000 --non-cs 0,10000 -d $full_duration --trials $trials --file-name asymmetric-contention
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
fetch_and_multiply

print "=== Full suite complete ==="
