#!/usr/bin/env python3
"""Read Arrow IPC files for each lock and produce a summary markdown table."""

import os
import pyarrow.ipc as ipc

OUTPUT_DIR = "visualization/output"
LOCKS = [
    "FC", "FCBan", "CC", "CCBan", "DSM",
    "FC_PQ_BHeap", "FC_PQ_BTree",
    "MCS", "Mutex", "SpinLock", "USCL",
    "C_FC", "C_CC", "C_AQS",
    "ShflLock", "ShflLock_C",
]
FILENAME = "counter cs [1000, 3000] noncs [0].arrow"

results = []

for lock in LOCKS:
    path = os.path.join(OUTPUT_DIR, lock, FILENAME)
    if not os.path.exists(path):
        print(f"WARNING: missing file for {lock}: {path}")
        continue

    reader = ipc.open_file(path)
    table = reader.read_all()

    locktype_vals = table.column("locktype").to_pylist()
    locktype = sorted(set(locktype_vals))[0]  # should be uniform

    loop_counts = table.column("loop_count").to_pylist()
    total_loop_count = sum(loop_counts)

    jfi_vals = table.column("jfi").to_pylist()
    # JFI varies by thread-count group; report min/max across the file
    jfi_min = min(jfi_vals)
    jfi_max = max(jfi_vals)

    num_rows = table.num_rows

    cs_lengths = sorted(set(table.column("cs_length").to_pylist()))

    thread_nums = sorted(set(table.column("thread_num").to_pylist()))

    results.append({
        "locktype": locktype,
        "total_loop_count": total_loop_count,
        "jfi_min": jfi_min,
        "jfi_max": jfi_max,
        "num_rows": num_rows,
        "cs_lengths": cs_lengths,
        "thread_nums": thread_nums,
    })

# Sort by total_loop_count descending
results.sort(key=lambda r: r["total_loop_count"], reverse=True)

# Print markdown table
header = "| Lock Type | Total Loop Count | JFI (min) | JFI (max) | Rows | Thread Configs | CS Lengths |"
sep    = "|-----------|-----------------|-----------|-----------|------|----------------|------------|"
print(header)
print(sep)
for r in results:
    threads_str = ", ".join(str(t) for t in r["thread_nums"])
    cs_str = ", ".join(str(c) for c in r["cs_lengths"])
    print(
        f"| {r['locktype']:<9s} "
        f"| {r['total_loop_count']:>15,} "
        f"| {r['jfi_min']:.6f}  "
        f"| {r['jfi_max']:.6f}  "
        f"| {r['num_rows']:>4} "
        f"| {threads_str:<14s} "
        f"| {cs_str:<10s} |"
    )
