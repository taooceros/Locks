#!/usr/bin/env nu
# perf stat profiling for all DLock2 lock variants.
# Collects cache, TLB, and migration events.
#
# Usage: nu profile.nu
# Output: profiles/dlock2-{lock}.txt (stdout) and profiles/dlock2-{lock}.stats (perf)

let dlock2 = "target/release/dlock"
let simple_cs = "1000,3000"
let duration = 5
let thread_num = 32

let perf_events = [
    cache-references,
    cache-misses,
    L1-dcache-loads,
    L1-dcache-load-misses,
    L1-dcache-stores,
    L1-dcache-store-misses,
    l2_rqsts.demand_data_rd_hit,
    l2_rqsts.demand_data_rd_miss,
    LLC-loads,
    LLC-load-misses,
    dTLB-loads,
    dTLB-load-misses,
    dTLB-stores,
    dTLB-store-misses,
    cpu-migrations,
    branch-misses,
    instructions,
    cycles,
]

let perf_arg = ($perf_events | str join ',')

let locks = [
    "fc",
    "fc-ban",
    "fc-pq-b-heap",
    "cc",
    "cc-ban",
    "dsm",
    "mcs",
    "mutex",
    "spin-lock",
    "uscl",
    "fc-c",
    "cc-c",
    "shfl-lock",
    "shfl-lock-c",
]

mkdir profiles

for lock in $locks {
    print $"Profiling ($lock)..."
    perf stat -e $perf_arg $dlock2 d-lock2 -t $thread_num --lock-targets $lock counter-proportional --cs $simple_cs --non-cs 0 -d $duration out> $"profiles/dlock2-($lock).txt" err> $"profiles/dlock2-($lock).stats"
}

print "Profiling complete. Results in profiles/"
