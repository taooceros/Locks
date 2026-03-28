#!/usr/bin/env python3
"""Analyze CFL fairness degradation over duration.

Reads the cfl-duration-sweep-{5,15,30,60}s.arrow files for CFL and FC_PQ_BHeap,
and prints a table of JFI per trial and per duration to show whether CFL's
fairness degrades as vLHT counters saturate.
"""

import os
import pyarrow.ipc as ipc
import statistics

BASE_DIR = "/home/hongtao/Locks/visualization/output"
LOCKS = ["CFL", "FC_PQ_BHeap"]
DURATIONS = [5, 15, 30, 60]


def load_arrow(lock_dir, filename):
    """Load Arrow IPC file, return list of (thread_num, loop_count, jfi, trial) rows."""
    path = os.path.join(BASE_DIR, lock_dir, f"{filename}.arrow")
    if not os.path.exists(path):
        return None
    with open(path, "rb") as f:
        table = ipc.open_file(f).read_all()

    cols = table.column_names
    rows = []
    for i in range(table.num_rows):
        row = {c: table.column(c)[i].as_py() for c in cols}
        rows.append(row)
    return rows


def extract_jfi_per_trial(rows):
    """Extract per-trial JFI values (JFI is same for all threads in a trial)."""
    if rows is None:
        return []

    # Group by trial if available, otherwise by JFI distinct values
    if "trial" in rows[0]:
        trials = {}
        for r in rows:
            t = r["trial"]
            if t not in trials:
                trials[t] = r["jfi"]
        return list(trials.values())
    else:
        # Fallback: JFI is per thread_num, collect unique values
        jfis = set()
        for r in rows:
            jfis.add(r["jfi"])
        return sorted(jfis)


def extract_throughput_per_trial(rows):
    """Extract per-trial total throughput."""
    if rows is None:
        return []

    if "trial" in rows[0]:
        trials = {}
        for r in rows:
            t = r["trial"]
            if t not in trials:
                trials[t] = 0
            trials[t] += r["loop_count"]
        return list(trials.values())
    else:
        # Sum all loop_count
        return [sum(r["loop_count"] for r in rows)]


def main():
    print("# CFL Fairness Duration Sweep Analysis")
    print()
    print("Hypothesis: CFL's vLHT counters saturate over time, causing")
    print("need_switch() to always return 'balanced' and disabling fairness.")
    print("FC-PQ (control) should maintain stable JFI regardless of duration.")
    print()

    # Collect data
    for lock in LOCKS:
        print(f"## {lock}")
        print()
        print(f"| Duration | Trial 1 | Trial 2 | Trial 3 | Trial 4 | Trial 5 | Mean | Std | Min |")
        print(f"|----------|---------|---------|---------|---------|---------|------|-----|-----|")

        for dur in DURATIONS:
            filename = f"cfl-duration-sweep-{dur}s"
            rows = load_arrow(lock, filename)
            jfis = extract_jfi_per_trial(rows)

            if not jfis:
                print(f"| {dur:>3}s     | {'N/A':>7} | {'N/A':>7} | {'N/A':>7} | {'N/A':>7} | {'N/A':>7} | {'N/A':>4} | {'N/A':>3} | {'N/A':>3} |")
                continue

            # Pad to 5 trials
            while len(jfis) < 5:
                jfis.append(float("nan"))

            trial_strs = [f"{j:.4f}" if j == j else "N/A" for j in jfis[:5]]
            valid = [j for j in jfis[:5] if j == j]  # filter NaN
            mean_jfi = statistics.mean(valid) if valid else float("nan")
            std_jfi = statistics.stdev(valid) if len(valid) > 1 else 0.0
            min_jfi = min(valid) if valid else float("nan")

            print(f"| {dur:>3}s     | {trial_strs[0]:>7} | {trial_strs[1]:>7} | {trial_strs[2]:>7} | {trial_strs[3]:>7} | {trial_strs[4]:>7} | {mean_jfi:.4f} | {std_jfi:.4f} | {min_jfi:.4f} |")

        print()

    # Throughput comparison
    print("## Throughput (millions of operations)")
    print()
    for lock in LOCKS:
        print(f"### {lock}")
        print()
        print(f"| Duration | Trial 1 | Trial 2 | Trial 3 | Trial 4 | Trial 5 | Mean |")
        print(f"|----------|---------|---------|---------|---------|---------|------|")

        for dur in DURATIONS:
            filename = f"cfl-duration-sweep-{dur}s"
            rows = load_arrow(lock, filename)
            tputs = extract_throughput_per_trial(rows)

            if not tputs:
                print(f"| {dur:>3}s     | {'N/A':>7} | {'N/A':>7} | {'N/A':>7} | {'N/A':>7} | {'N/A':>7} | {'N/A':>6} |")
                continue

            while len(tputs) < 5:
                tputs.append(float("nan"))

            trial_strs = [f"{t/1e6:.1f}" if t == t else "N/A" for t in tputs[:5]]
            valid = [t for t in tputs[:5] if t == t]
            mean_t = statistics.mean(valid) if valid else float("nan")

            print(f"| {dur:>3}s     | {trial_strs[0]:>7} | {trial_strs[1]:>7} | {trial_strs[2]:>7} | {trial_strs[3]:>7} | {trial_strs[4]:>7} | {mean_t/1e6:.1f}M |")

        print()


if __name__ == "__main__":
    main()
