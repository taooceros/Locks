cd rust

let simple_cs = "1,3"
let dlock2 = "target/release/dlock"

let args = ($"d-lock2 --lock-targets fc counter-proportional --cs ($simple_cs) --non-cs 0 -d 5")

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
]

let duration = 1

let perf_arg = ($perf_events | str join ',')

let locks = [
    "fc",
    "cc",
    "cc-ban",
    "fc-ban",
    "fc-sl",
    "fc-pq-b-heap",
    "fc-pq-b-tree",
    "spin-lock",
    "uscl",
    "mutex"
]

let thread_num = 36

for lock in $locks {
    perf stat -e $perf_arg $dlock2 d-lock2 -t $thread_num --lock-targets $lock counter-proportional --cs ($simple_cs) --non-cs 0 -d $duration out> $"../profiles/dlock2-($lock).txt" err> $"../profiles/dlock2-($lock).stats"
}

