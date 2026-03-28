#!/usr/bin/env python3
"""Read Arrow IPC files for multiple lock types and print markdown tables."""

import pyarrow.ipc as ipc
from collections import defaultdict
import os

BASE_DIR = "/home/hongtao/Locks/visualization/output"

LOCKS = [
    ("C_AQS", "C_AQS (Linux kernel MCS/AQS spinlock)"),
    ("FC", "FC (Flat Combining, unfair delegation)"),
    ("FC_PQ_BHeap", "FC_PQ_BHeap (Flat Combining with Priority Queue, fair delegation)"),
    ("MCS", "MCS (MCS queue lock)"),
]

FILENAME = "counter cs [1000, 3000] noncs [0].arrow"


def read_and_print(lock_dir: str, lock_label: str):
    filepath = os.path.join(BASE_DIR, lock_dir, FILENAME)
    if not os.path.exists(filepath):
        print(f"## {lock_label}\n")
        print(f"**File not found:** `{filepath}`\n")
        return

    reader = ipc.open_file(filepath)
    table = reader.read_all()

    thread_nums = table.column("thread_num").to_pylist()
    cs_lengths = table.column("cs_length").to_pylist()
    loop_counts = table.column("loop_count").to_pylist()
    jfis = table.column("jfi").to_pylist()

    # Group by (thread_num, cs_length)
    groups = defaultdict(lambda: {"total_loop_count": 0, "jfi_values": set()})
    for i in range(table.num_rows):
        key = (thread_nums[i], cs_lengths[i])
        groups[key]["total_loop_count"] += loop_counts[i]
        groups[key]["jfi_values"].add(jfis[i])

    # Sort by thread_num, then cs_length
    sorted_keys = sorted(groups.keys())

    print(f"## {lock_label}\n")
    print("| thread_num | cs_length | total_loop_count | JFI |")
    print("|------------|-----------|------------------|-----|")
    for tn, cs in sorted_keys:
        g = groups[(tn, cs)]
        total_lc = g["total_loop_count"]
        # JFI should be the same for all rows in a thread_num group
        jfi_val = next(iter(g["jfi_values"]))
        print(f"| {tn:>10} | {cs:>9} | {total_lc:>16,} | {jfi_val:.6f} |")
    print()


def main():
    for lock_dir, lock_label in LOCKS:
        read_and_print(lock_dir, lock_label)


if __name__ == "__main__":
    main()
