#!/usr/bin/env python3
"""Summarize CS ratio sweep results: throughput and JFI per lock × thread_count × ratio."""

import os
import pyarrow.ipc as ipc

BASE_DIR = "/home/hongtao/Locks/visualization/output"

LOCKS = [
    "FC", "FCBan", "CC", "CCBan", "DSM",
    "FC_PQ_BTree", "FC_PQ_BHeap",
    "Mutex", "SpinLock", "USCL",
    "C_FC", "C_CC", "MCS",
    "ShflLock", "ShflLock_C",
]
RATIOS = [
    ("ratio-1-1", "1:1"),
    ("ratio-1-3", "1:3"),
    ("ratio-1-10", "1:10"),
    ("ratio-1-30", "1:30"),
    ("ratio-1-100", "1:100"),
]
THREAD_COUNTS = [4, 16, 64]


def load(lock, filename):
    path = os.path.join(BASE_DIR, lock, f"{filename}.arrow")
    if not os.path.exists(path):
        return {}
    with open(path, "rb") as f:
        table = ipc.open_file(f).read_all()
    tns = table.column("thread_num").to_pylist()
    lcs = table.column("loop_count").to_pylist()
    jfis = table.column("jfi").to_pylist()
    result = {}
    for tn, lc, jfi in zip(tns, lcs, jfis):
        if tn not in result:
            result[tn] = {"lc": 0, "jfi": jfi}
        result[tn]["lc"] += lc
    return result


def fmt_m(val):
    return f"{val // 1_000_000:,}"


def main():
    # JFI table
    print("## JFI by Lock × Ratio × Thread Count\n")
    for tc in THREAD_COUNTS:
        print(f"### {tc} threads\n")
        header = "| Lock" + "".join(f" | {label}" for _, label in RATIOS) + " |"
        sep = "|---" + "|---:" * len(RATIOS) + "|"
        print(header)
        print(sep)
        for lock in LOCKS:
            cells = []
            for fname, _ in RATIOS:
                data = load(lock, fname)
                if tc in data:
                    cells.append(f"{data[tc]['jfi']:.4f}")
                else:
                    cells.append("N/A")
            print(f"| {lock:<12s} | " + " | ".join(cells) + " |")
        print()

    # Throughput table
    print("## Throughput (millions) by Lock × Ratio × Thread Count\n")
    for tc in THREAD_COUNTS:
        print(f"### {tc} threads\n")
        header = "| Lock" + "".join(f" | {label}" for _, label in RATIOS) + " |"
        sep = "|---" + "|---:" * len(RATIOS) + "|"
        print(header)
        print(sep)
        for lock in LOCKS:
            cells = []
            for fname, _ in RATIOS:
                data = load(lock, fname)
                if tc in data:
                    cells.append(fmt_m(data[tc]["lc"]))
                else:
                    cells.append("N/A")
            print(f"| {lock:<12s} | " + " | ".join(cells) + " |")
        print()


if __name__ == "__main__":
    main()
