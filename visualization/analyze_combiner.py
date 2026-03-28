#!/usr/bin/env python3
"""Phase 2 analysis: combiner response-time study.

Reads Arrow IPC files from group_combiner experiments and computes:
1. Combiner penalty ratio: combiner_p99 / waiter_p50
2. Combiner time fraction per thread (combine_time / total)
3. Combiner vs waiter percentile comparison

Usage:
    python visualization/analyze_combiner.py [--file-pattern PATTERN]

Example:
    python visualization/analyze_combiner.py --file-pattern "combiner-study-*"
    python visualization/analyze_combiner.py --file-pattern "latency-cs1000-3000"
"""

import argparse
import os
import sys
from collections import defaultdict
from pathlib import Path

import numpy as np
import pyarrow.ipc as ipc

BASE_DIR = Path(__file__).parent / "output"

LOCKS = [
    "FC", "FC_PQ_BHeap", "FCBan", "ShflLock", "CC", "CCBan", "MCS", "CFL",
]

LOCK_LABELS = {
    "FC": "FC",
    "FC_PQ_BHeap": "FC-PQ",
    "FCBan": "FC-Ban",
    "ShflLock": "ShflLock",
    "CC": "CC",
    "CCBan": "CC-Ban",
    "MCS": "MCS",
    "CFL": "CFL",
}


def find_files(pattern: str):
    """Find Arrow files matching pattern across all lock directories."""
    import fnmatch

    filenames = set()
    for lock_dir in LOCKS:
        lock_path = BASE_DIR / lock_dir
        if not lock_path.exists():
            continue
        for f in lock_path.iterdir():
            if f.suffix == ".arrow" and fnmatch.fnmatch(f.stem, pattern):
                filenames.add(f.stem)
    return sorted(filenames)


def load_records(lock_dir: str, filename: str):
    """Load Arrow file, return per-thread records grouped by (thread_num, trial)."""
    path = BASE_DIR / lock_dir / f"{filename}.arrow"
    if not path.exists():
        return {}
    with open(path, "rb") as f:
        table = ipc.open_file(f).read_all()

    cols = table.column_names
    tns = table.column("thread_num").to_pylist()
    trials = table.column("trial").to_pylist()

    combiner_lats = table.column("combiner_latency").to_pylist() if "combiner_latency" in cols else [[] for _ in tns]
    waiter_lats = table.column("waiter_latency").to_pylist() if "waiter_latency" in cols else [[] for _ in tns]
    combine_times = table.column("combine_time").to_pylist() if "combine_time" in cols else [None for _ in tns]
    hold_times = table.column("hold_time").to_pylist() if "hold_time" in cols else [0 for _ in tns]

    groups = defaultdict(list)
    for i in range(table.num_rows):
        key = (tns[i], trials[i])
        groups[key].append({
            "combiner_latency": combiner_lats[i] or [],
            "waiter_latency": waiter_lats[i] or [],
            "combine_time": combine_times[i],
            "hold_time": hold_times[i],
        })
    return groups


def percentile(arr, p):
    """Compute percentile from sorted array."""
    if len(arr) == 0:
        return 0
    idx = int(len(arr) * p / 100.0)
    return arr[min(idx, len(arr) - 1)]


def analyze_penalty(records_by_key):
    """Compute combiner penalty metrics per (thread_num, trial)."""
    results = {}
    for (tn, trial), records in records_by_key.items():
        all_combiner = sorted([v for r in records for v in r["combiner_latency"]])
        all_waiter = sorted([v for r in records for v in r["waiter_latency"]])

        if not all_combiner or not all_waiter:
            continue

        c_p50 = percentile(all_combiner, 50)
        c_p95 = percentile(all_combiner, 95)
        c_p99 = percentile(all_combiner, 99)
        w_p50 = percentile(all_waiter, 50)
        w_p95 = percentile(all_waiter, 95)
        w_p99 = percentile(all_waiter, 99)

        penalty_ratio = c_p99 / w_p50 if w_p50 > 0 else float("inf")

        # Combiner time fraction per thread
        combine_times = [r["combine_time"] for r in records if r["combine_time"] is not None]
        total_hold = sum(r["hold_time"] for r in records)

        results[(tn, trial)] = {
            "combiner_p50": c_p50,
            "combiner_p95": c_p95,
            "combiner_p99": c_p99,
            "waiter_p50": w_p50,
            "waiter_p95": w_p95,
            "waiter_p99": w_p99,
            "penalty_ratio": penalty_ratio,
            "n_combiner_samples": len(all_combiner),
            "n_waiter_samples": len(all_waiter),
            "combine_times": combine_times,
            "total_hold": total_hold,
        }
    return results


def print_penalty_table(all_results, filename):
    """Print markdown table of combiner penalty ratios."""
    print(f"\n## Combiner Penalty: {filename}\n")
    print("| Lock | Threads | Combiner p99 | Waiter p50 | Penalty Ratio | Combiner% | Waiter% |")
    print("|------|--------:|-------------:|-----------:|--------------:|----------:|--------:|")

    for lock_dir in LOCKS:
        if lock_dir not in all_results:
            continue
        results = all_results[lock_dir]
        label = LOCK_LABELS.get(lock_dir, lock_dir)

        # Average across trials for each thread count
        by_tn = defaultdict(list)
        for (tn, trial), data in results.items():
            by_tn[tn].append(data)

        for tn in sorted(by_tn.keys()):
            trials = by_tn[tn]
            avg_c99 = np.mean([t["combiner_p99"] for t in trials])
            avg_w50 = np.mean([t["waiter_p50"] for t in trials])
            avg_penalty = np.mean([t["penalty_ratio"] for t in trials])
            avg_c_pct = np.mean([
                t["n_combiner_samples"] / (t["n_combiner_samples"] + t["n_waiter_samples"]) * 100
                for t in trials if t["n_combiner_samples"] + t["n_waiter_samples"] > 0
            ])
            avg_w_pct = 100 - avg_c_pct

            print(f"| {label:<6s} | {tn:>7d} | {avg_c99:>12,.0f} | {avg_w50:>10,.0f} | "
                  f"{avg_penalty:>13.1f}x | {avg_c_pct:>8.1f}% | {avg_w_pct:>6.1f}% |")


def print_combine_time_table(all_results, filename):
    """Print combine_time distribution per thread (for delegation locks only)."""
    print(f"\n## Combiner Time Distribution: {filename}\n")
    print("| Lock | Threads | Mean Combine Time | Std Dev | Max/Mean |")
    print("|------|--------:|------------------:|--------:|---------:|")

    for lock_dir in LOCKS:
        if lock_dir not in all_results:
            continue
        results = all_results[lock_dir]
        label = LOCK_LABELS.get(lock_dir, lock_dir)

        by_tn = defaultdict(list)
        for (tn, trial), data in results.items():
            if data["combine_times"]:
                by_tn[tn].extend(data["combine_times"])

        for tn in sorted(by_tn.keys()):
            times = by_tn[tn]
            if not times:
                continue
            arr = np.array(times, dtype=float)
            mean_ct = np.mean(arr)
            std_ct = np.std(arr)
            max_mean = np.max(arr) / mean_ct if mean_ct > 0 else 0

            print(f"| {label:<6s} | {tn:>7d} | {mean_ct:>17,.0f} | {std_ct:>7,.0f} | {max_mean:>8.2f}x |")


def print_penalty_scaling(all_results, filename):
    """Print combiner penalty ratio vs thread count for each lock."""
    print(f"\n## Penalty Scaling: {filename}\n")

    # Collect all thread counts
    all_tns = set()
    for lock_dir in LOCKS:
        if lock_dir not in all_results:
            continue
        for (tn, _) in all_results[lock_dir]:
            all_tns.add(tn)
    tns = sorted(all_tns)
    if not tns:
        return

    header = "| Lock |" + "".join(f" {tn}T |" for tn in tns)
    sep = "|------|" + "|------:" * len(tns) + "|"
    print(header)
    print(sep)

    for lock_dir in LOCKS:
        if lock_dir not in all_results:
            continue
        label = LOCK_LABELS.get(lock_dir, lock_dir)
        results = all_results[lock_dir]

        by_tn = defaultdict(list)
        for (tn, trial), data in results.items():
            by_tn[tn].append(data["penalty_ratio"])

        cells = []
        for tn in tns:
            if tn in by_tn:
                avg = np.mean(by_tn[tn])
                cells.append(f"{avg:.1f}x")
            else:
                cells.append("N/A")
        print(f"| {label:<6s} | " + " | ".join(cells) + " |")


def main():
    parser = argparse.ArgumentParser(description="Phase 2: combiner response-time analysis")
    parser.add_argument("--file-pattern", default="combiner-study-*",
                        help="Glob pattern for Arrow file stems (default: combiner-study-*)")
    args = parser.parse_args()

    filenames = find_files(args.file_pattern)
    if not filenames:
        print(f"No Arrow files matching '{args.file_pattern}' found in {BASE_DIR}")
        sys.exit(1)

    for filename in filenames:
        all_results = {}
        for lock_dir in LOCKS:
            records = load_records(lock_dir, filename)
            if records:
                all_results[lock_dir] = analyze_penalty(records)

        if not all_results:
            print(f"No data for {filename}")
            continue

        print_penalty_table(all_results, filename)
        print_combine_time_table(all_results, filename)

    # Cross-file penalty scaling table (aggregate all files)
    print("\n" + "=" * 60)
    print("# Penalty Scaling Across Thread Counts")
    combined_results = {}
    for filename in filenames:
        for lock_dir in LOCKS:
            records = load_records(lock_dir, filename)
            if records:
                analyzed = analyze_penalty(records)
                if lock_dir not in combined_results:
                    combined_results[lock_dir] = {}
                combined_results[lock_dir].update(analyzed)

    if combined_results:
        print_penalty_scaling(combined_results, "all-combiner-study")


if __name__ == "__main__":
    main()
