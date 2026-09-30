"""Figures and median tables for report.typ from the formal run's summary.json.

Usage (from the workspace root):
    python3 docs/evidence/redb-closure-rerun-2026-09-29/make_figures.py \
        .worktree/redb-rerun-06/analysis/summary.json
"""
import collections
import json
import pathlib
import statistics
import sys

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt

OUT = pathlib.Path(__file__).parent
VARIANTS = ["native", "refactored", "bridge_mutex", "mcs", "uscl", "fc", "fc_pq"]
CLIENTS = [1, 2, 4, 8]
METRICS = ["throughput_tx_s", "throughput_records_s", "service_jain", "long_service_share"]


def main(summary_path):
    rows = [r for r in json.load(open(summary_path))["rows"] if not r.get("failure")]
    groups = collections.defaultdict(list)
    for r in rows:
        for m in METRICS:
            if r.get(m) is not None:
                groups[(m, r["cohort"], r["durability"], r["variant"], r["clients"])].append(r[m])
    medians = {"|".join(map(str, k)): statistics.median(v) for k, v in groups.items()}
    (OUT / "medians.json").write_text(json.dumps(medians, indent=0, sort_keys=True))

    panels = [
        ("throughput_tx_s", "all1", "none", "tx/s: all1, durability None"),
        ("throughput_tx_s", "all1", "immediate", "tx/s: all1, durability Immediate"),
        ("throughput_records_s", "half1_half64", "none", "records/s: half1_half64, None"),
        ("service_jain", "half1_half64", "none", "service-time Jain: half1_half64, None"),
        ("long_service_share", "half1_half64", "none", "long-client service share: half1_half64, None"),
        ("service_jain", "half1_half64", "immediate", "service-time Jain: half1_half64, Immediate"),
    ]
    fig, axes = plt.subplots(2, 3, figsize=(16, 9))
    for ax, (m, cohort, dur, title) in zip(axes.flat, panels):
        for v in VARIANTS:
            pts = [(c, medians.get(f"{m}|{cohort}|{dur}|{v}|{c}")) for c in CLIENTS]
            pts = [(c, y) for c, y in pts if y is not None]
            if pts:
                ax.plot(*zip(*pts), marker="o", label=v, lw=2.5 if v == "fc_pq" else 1.2)
        ax.set_title(title)
        ax.set_xticks(CLIENTS)
        ax.set_xlabel("clients")
        ax.grid(alpha=0.3)
    axes[0, 0].legend(fontsize=8)
    fig.tight_layout()
    fig.savefig(OUT / "summary.png", dpi=150)


if __name__ == "__main__":
    main(sys.argv[1])
