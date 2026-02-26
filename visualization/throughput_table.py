#!/usr/bin/env python3
"""
Read Arrow IPC files for specified locks and produce:
  1. A markdown table of total loop_count (in millions) per lock x thread_count.
  2. A markdown table of throughput ratio relative to FC (FC = 1.0x).

Each file: visualization/output/{LOCK}/counter cs [1000, 3000] noncs [0].arrow

For each lock and thread_num, total loop_count = sum across all threads and
both cs_lengths in that thread_num group.
"""

import os
import pyarrow.ipc as ipc

BASE_DIR = "/home/hongtao/Locks/visualization/output"
FILE_NAME = "counter cs [1000, 3000] noncs [0].arrow"

LOCKS = [
    "FC", "CC", "DSM", "FC_PQ_BHeap", "FCBan", "CCBan",
    "MCS", "Mutex", "SpinLock", "USCL",
    "C_FC", "C_CC", "ShflLock", "ShflLock_C",
]

THREAD_COUNTS = [4, 8, 16, 32, 64, 128]


def load_totals(lock_name):
    """Return dict: thread_num -> total loop_count."""
    path = os.path.join(BASE_DIR, lock_name, FILE_NAME)
    if not os.path.exists(path):
        print(f"WARNING: file not found for {lock_name}: {path}")
        return {}
    with open(path, "rb") as f:
        reader = ipc.open_file(f)
        table = reader.read_all()

    thread_nums = table.column("thread_num").to_pylist()
    loop_counts = table.column("loop_count").to_pylist()

    result = {}
    for tn, lc in zip(thread_nums, loop_counts):
        result[tn] = result.get(tn, 0) + lc
    return result


def load_jfi(lock_name):
    """Return dict: thread_num -> average JFI across rows for that thread_num."""
    path = os.path.join(BASE_DIR, lock_name, FILE_NAME)
    if not os.path.exists(path):
        return {}
    with open(path, "rb") as f:
        reader = ipc.open_file(f)
        table = reader.read_all()

    thread_nums = table.column("thread_num").to_pylist()
    jfis = table.column("jfi").to_pylist()

    # JFI is the same for all rows in a thread_num group; take the first non-zero.
    result = {}
    for tn, jfi in zip(thread_nums, jfis):
        if tn not in result and jfi is not None and jfi > 0:
            result[tn] = jfi
    return result


def fmt_millions(val):
    """Format value in millions as integer with comma separators."""
    m = val // 1_000_000
    return f"{m:,}"


def main():
    # ── Collect data ──
    data = {}
    jfi_data = {}
    for lock in LOCKS:
        data[lock] = load_totals(lock)
        jfi_data[lock] = load_jfi(lock)

    # Only include locks that have data
    available_locks = [l for l in LOCKS if data[l]]

    # Sort by 128-thread throughput descending
    sorted_locks = sorted(
        available_locks,
        key=lambda lk: data[lk].get(128, 0),
        reverse=True,
    )

    # ── Table 1: Absolute throughput in millions ──
    name_w = max(len(l) for l in sorted_locks)
    col_w = {}
    for tn in THREAD_COUNTS:
        vals = [fmt_millions(data[l][tn]) if tn in data[l] else "N/A"
                for l in sorted_locks]
        col_w[tn] = max(len(f"{tn}T"), max(len(v) for v in vals))

    def make_row(name_cell, value_cells):
        parts = [f"{name_cell:<{name_w}}"]
        for tn, cell in zip(THREAD_COUNTS, value_cells):
            parts.append(f"{cell:>{col_w[tn]}}")
        return "| " + " | ".join(parts) + " |"

    def make_sep():
        parts = ["-" * name_w + "-"]
        for tn in THREAD_COUNTS:
            parts.append("-" * col_w[tn] + ":")
        return "|-" + "-|-".join(parts) + "|"

    print("## Total Throughput (loop_count in millions)")
    print()
    print(make_row("Lock", [f"{tn}T" for tn in THREAD_COUNTS]))
    print(make_sep())
    for lock in sorted_locks:
        vals = []
        for tn in THREAD_COUNTS:
            v = data[lock].get(tn, None)
            vals.append(fmt_millions(v) if v is not None else "N/A")
        print(make_row(lock, vals))

    print()

    # ── Table 2: Ratio relative to FC ──
    fc_data = data.get("FC", {})
    if not fc_data:
        print("ERROR: FC data not found, cannot compute ratios.")
        return

    ratio_w = {}
    for tn in THREAD_COUNTS:
        ratio_w[tn] = max(len(f"{tn}T"), 5)  # "X.XXx" = 5 chars

    def make_row2(name_cell, value_cells):
        parts = [f"{name_cell:<{name_w}}"]
        for tn, cell in zip(THREAD_COUNTS, value_cells):
            parts.append(f"{cell:>{ratio_w[tn]}}")
        return "| " + " | ".join(parts) + " |"

    def make_sep2():
        parts = ["-" * name_w + "-"]
        for tn in THREAD_COUNTS:
            parts.append("-" * ratio_w[tn] + ":")
        return "|-" + "-|-".join(parts) + "|"

    print("## Throughput Ratio (relative to FC = 1.00x)")
    print()
    print(make_row2("Lock", [f"{tn}T" for tn in THREAD_COUNTS]))
    print(make_sep2())
    for lock in sorted_locks:
        vals = []
        for tn in THREAD_COUNTS:
            lk_val = data[lock].get(tn, None)
            fc_val = fc_data.get(tn, None)
            if lk_val is None or fc_val is None or fc_val == 0:
                vals.append("  N/A")
            else:
                ratio = lk_val / fc_val
                vals.append(f"{ratio:.2f}x")
        print(make_row2(lock, vals))


    # ── Table 3: JFI (Jain's Fairness Index) ──
    print()
    print("## Jain's Fairness Index (JFI)")
    print()

    jfi_w = {}
    for tn in THREAD_COUNTS:
        jfi_w[tn] = max(len(f"{tn}T"), 6)  # "X.XXXX" = 6 chars

    def make_row3(name_cell, value_cells):
        parts = [f"{name_cell:<{name_w}}"]
        for tn, cell in zip(THREAD_COUNTS, value_cells):
            parts.append(f"{cell:>{jfi_w[tn]}}")
        return "| " + " | ".join(parts) + " |"

    def make_sep3():
        parts = ["-" * name_w + "-"]
        for tn in THREAD_COUNTS:
            parts.append("-" * jfi_w[tn] + ":")
        return "|-" + "-|-".join(parts) + "|"

    print(make_row3("Lock", [f"{tn}T" for tn in THREAD_COUNTS]))
    print(make_sep3())
    for lock in sorted_locks:
        vals = []
        for tn in THREAD_COUNTS:
            jfi = jfi_data[lock].get(tn, None)
            if jfi is not None:
                vals.append(f"{jfi:.4f}")
            else:
                vals.append("   N/A")
        print(make_row3(lock, vals))


if __name__ == "__main__":
    main()
