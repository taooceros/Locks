#!/usr/bin/env python3
"""Pareto scatter plot: throughput vs JFI for delegation fairness benefits.

Reads Arrow IPC files from group_benefits / group_tradeoff experiments.
Produces scatter plots showing FC-PQ in the high-throughput, high-fairness
corner of the Pareto space.

Usage:
    python visualization/plot_pareto.py [--file-pattern PATTERN] [--output DIR]

Example:
    python visualization/plot_pareto.py --file-pattern "benefits-32t-cs-*"
    python visualization/plot_pareto.py --file-pattern "tradeoff-3to1-32t"
"""

import argparse
import os
import sys
from collections import defaultdict
from pathlib import Path

import pyarrow.ipc as ipc

BASE_DIR = Path(__file__).parent / "output"

# Lock directory names (from DLock2Impl Display trait)
LOCKS = {
    "FC": {"label": "FC", "color": "#e41a1c", "marker": "o"},
    "FC_PQ_BHeap": {"label": "FC-PQ", "color": "#377eb8", "marker": "s"},
    "FCBan": {"label": "FC-Ban", "color": "#4daf4a", "marker": "^"},
    "ShflLock": {"label": "ShflLock", "color": "#e7298a", "marker": "P"},
    "MCS": {"label": "MCS", "color": "#984ea3", "marker": "D"},
    "CFL": {"label": "CFL", "color": "#ff7f00", "marker": "v"},
    "Mutex": {"label": "Mutex", "color": "#a65628", "marker": "p"},
    "SpinLock": {"label": "SpinLock", "color": "#f781bf", "marker": "h"},
    "Ticket": {"label": "Ticket", "color": "#999999", "marker": "<"},
    "CLH": {"label": "CLH", "color": "#66c2a5", "marker": ">"},
}


def load_arrow(lock_dir: str, filename: str):
    """Load Arrow IPC file, return list of (thread_num, trial, loop_count, jfi) tuples."""
    path = BASE_DIR / lock_dir / f"{filename}.arrow"
    if not path.exists():
        return []
    with open(path, "rb") as f:
        table = ipc.open_file(f).read_all()
    rows = []
    tns = table.column("thread_num").to_pylist()
    trials = table.column("trial").to_pylist()
    lcs = table.column("loop_count").to_pylist()
    jfis = table.column("jfi").to_pylist()
    for tn, trial, lc, jfi in zip(tns, trials, lcs, jfis):
        rows.append((tn, trial, lc, jfi))
    return rows


def aggregate(rows):
    """Aggregate rows: sum loop_count per (thread_num, trial), take JFI value.
    Returns dict: thread_num -> list of (total_throughput, jfi) per trial."""
    groups = defaultdict(lambda: defaultdict(lambda: {"lc": 0, "jfi": 0.0}))
    for tn, trial, lc, jfi in rows:
        groups[tn][trial]["lc"] += lc
        groups[tn][trial]["jfi"] = jfi  # same for all threads in a group
    result = {}
    for tn, trials in groups.items():
        result[tn] = [(t["lc"], t["jfi"]) for t in trials.values()]
    return result


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


def print_table(data, filename):
    """Print markdown table of throughput and JFI per lock."""
    print(f"\n## {filename}\n")
    print("| Lock | Throughput (M ops) | JFI | Relative Throughput |")
    print("|------|-------------------:|----:|--------------------:|")

    # Find FC throughput for normalization
    fc_throughput = None
    for lock_dir, vals in data.items():
        if lock_dir == "FC" and vals:
            fc_throughput = sum(v[0] for v in vals) / len(vals)
            break

    rows = []
    for lock_dir in LOCKS:
        if lock_dir not in data or not data[lock_dir]:
            continue
        vals = data[lock_dir]
        avg_tp = sum(v[0] for v in vals) / len(vals)
        avg_jfi = sum(v[1] for v in vals) / len(vals)
        rel = avg_tp / fc_throughput if fc_throughput else 0
        rows.append((lock_dir, avg_tp, avg_jfi, rel))

    rows.sort(key=lambda r: -r[1])  # sort by throughput descending
    for lock_dir, avg_tp, avg_jfi, rel in rows:
        label = LOCKS[lock_dir]["label"]
        print(f"| {label:<10s} | {avg_tp / 1e6:>17.2f} | {avg_jfi:.4f} | {rel:>19.2f}x |")


def plot_scatter(data, filename, output_dir):
    """Generate matplotlib scatter plot."""
    try:
        import matplotlib.pyplot as plt
        import matplotlib
        matplotlib.use("Agg")
    except ImportError:
        print("matplotlib not installed, skipping plot generation", file=sys.stderr)
        return

    fig, ax = plt.subplots(figsize=(8, 6))

    fc_tp = None
    points = {}
    for lock_dir in LOCKS:
        if lock_dir not in data or not data[lock_dir]:
            continue
        vals = data[lock_dir]
        avg_tp = sum(v[0] for v in vals) / len(vals)
        avg_jfi = sum(v[1] for v in vals) / len(vals)
        if lock_dir == "FC":
            fc_tp = avg_tp
        points[lock_dir] = (avg_tp, avg_jfi)

    if not fc_tp:
        print("No FC data found, cannot normalize", file=sys.stderr)
        return

    # Plot each lock
    for lock_dir, (tp, jfi) in points.items():
        info = LOCKS[lock_dir]
        rel_tp = tp / fc_tp
        ax.scatter(rel_tp, jfi, c=info["color"], marker=info["marker"],
                   s=120, label=info["label"], zorder=5, edgecolors="black", linewidths=0.5)
        ax.annotate(info["label"], (rel_tp, jfi), textcoords="offset points",
                    xytext=(8, 4), fontsize=9)

    # Draw arrows: FC -> FC-PQ and MCS -> CFL
    for src, dst, color in [("FC", "FC_PQ_BHeap", "#377eb8"), ("MCS", "CFL", "#ff7f00")]:
        if src in points and dst in points:
            x0, y0 = points[src][0] / fc_tp, points[src][1]
            x1, y1 = points[dst][0] / fc_tp, points[dst][1]
            ax.annotate("", xy=(x1, y1), xytext=(x0, y0),
                        arrowprops=dict(arrowstyle="->", color=color, lw=2, ls="--"))

    ax.set_xlabel("Throughput (relative to FC)", fontsize=12)
    ax.set_ylabel("Jain's Fairness Index", fontsize=12)
    ax.set_title(f"Fairness-Throughput Pareto: {filename}", fontsize=13)
    ax.set_ylim(0.45, 1.02)
    ax.set_xlim(0, 1.15)
    ax.grid(True, alpha=0.3)
    ax.legend(loc="lower left", fontsize=9)

    out_path = Path(output_dir) / f"pareto-{filename}.png"
    fig.savefig(out_path, dpi=150, bbox_inches="tight")
    print(f"Saved plot to {out_path}")
    plt.close()


def main():
    parser = argparse.ArgumentParser(description="Pareto scatter plot: throughput vs JFI")
    parser.add_argument("--file-pattern", default="benefits-32t-cs-*",
                        help="Glob pattern for Arrow file stems (default: benefits-32t-cs-*)")
    parser.add_argument("--output", default="visualization/plots",
                        help="Output directory for plots (default: visualization/plots)")
    parser.add_argument("--thread-num", type=int, default=None,
                        help="Filter to specific thread count (default: all)")
    args = parser.parse_args()

    os.makedirs(args.output, exist_ok=True)

    filenames = find_files(args.file_pattern)
    if not filenames:
        print(f"No Arrow files matching '{args.file_pattern}' found in {BASE_DIR}")
        sys.exit(1)

    for filename in filenames:
        data = {}
        for lock_dir in LOCKS:
            rows = load_arrow(lock_dir, filename)
            if not rows:
                continue
            agg = aggregate(rows)
            if args.thread_num:
                if args.thread_num in agg:
                    data[lock_dir] = agg[args.thread_num]
            else:
                # Use the largest thread count available
                max_tn = max(agg.keys())
                data[lock_dir] = agg[max_tn]

        if not data:
            print(f"No data for {filename}")
            continue

        print_table(data, filename)
        plot_scatter(data, filename, args.output)


if __name__ == "__main__":
    main()
