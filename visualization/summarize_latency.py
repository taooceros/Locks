#!/usr/bin/env python3
"""Extract response time percentiles from latency Arrow files (no numpy)."""

import os
import math
import pyarrow.ipc as ipc

BASE = "/home/hongtao/Locks/visualization/output"
LOCKS = [
    "FC", "FCBan", "CC", "CCBan", "DSM",
    "FC_PQ_BTree", "FC_PQ_BHeap",
    "Mutex", "SpinLock", "USCL",
    "C_FC", "C_CC", "MCS",
    "ShflLock", "ShflLock_C",
]
FILE = "latency-cs1000-3000.arrow"


def percentile(sorted_data, p):
    """Compute p-th percentile from sorted list."""
    n = len(sorted_data)
    k = (p / 100.0) * (n - 1)
    f = math.floor(k)
    c = math.ceil(k)
    if f == c:
        return sorted_data[int(k)]
    return sorted_data[f] * (c - k) + sorted_data[c] * (k - f)


print("## Response Time Percentiles (TSC cycles)")
print()

for tc in [8, 32]:
    print(f"### {tc} threads")
    print()
    print("| Lock | Role | p50 | p95 | p99 | p99.9 |")
    print("|------|------|----:|----:|----:|------:|")

    for lock in LOCKS:
        path = os.path.join(BASE, lock, FILE)
        if not os.path.exists(path):
            continue
        table = ipc.open_file(path).read_all()
        tns = table.column("thread_num").to_pylist()
        combiner_lats = []
        waiter_lats = []
        for i in range(table.num_rows):
            if tns[i] != tc:
                continue
            cl = table.column("combiner_latency")[i].as_py()
            wl = table.column("waiter_latency")[i].as_py()
            if cl:
                combiner_lats.extend(cl)
            if wl:
                waiter_lats.extend(wl)

        for role, lats in [("combiner", combiner_lats), ("waiter", waiter_lats)]:
            if not lats:
                continue
            lats.sort()
            p50 = percentile(lats, 50)
            p95 = percentile(lats, 95)
            p99 = percentile(lats, 99)
            p999 = percentile(lats, 99.9)
            print(
                f"| {lock:<12s} | {role:<8s} "
                f"| {int(p50):>8,} | {int(p95):>8,} "
                f"| {int(p99):>8,} | {int(p999):>10,} |"
            )
    print()
