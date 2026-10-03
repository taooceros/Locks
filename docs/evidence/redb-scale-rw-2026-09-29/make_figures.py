"""Medians and figures for report.typ from the scale_rw analyses.

Usage (from the workspace root):
    python3 docs/evidence/redb-scale-rw-2026-09-29/make_figures.py .worktree/redb-scale-rw-01

Reads <root>/analysis-{scale,rw,perf}/summary.json and writes medians.json (flat
"kind|...|metric|variant" keys, medians over the repeats of non-failed cells) and
scale.png, readers.png, clock.png next to this file.
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
VARIANTS = ["upstream", "upstream_gate", "std_mutex", "mcs", "uscl", "fc", "fc_pq"]
STYLE = {  # requester-run locks solid, combiners dashed, controls dotted
    "upstream": ("#888888", ":"), "upstream_gate": ("#444444", ":"), "std_mutex": ("#aa7700", ":"),
    "mcs": ("#1f77b4", "-"), "uscl": ("#2ca02c", "-"), "fc": ("#d62728", "--"), "fc_pq": ("#9467bd", "--"),
}
SCALE_W = [1, 2, 4, 8, 16, 32, 64]
SCALE_METRICS = ["throughput_tx_s", "throughput_records_s", "service_jain", "tx_jain", "long_service_share",
                 "clock_ghz", "tx_s_at_ref", "response_p99_ms_upper", "service_utilization"]
RW_METRICS = ["throughput_tx_s", "service_jain", "tx_jain", "clock_ghz", "reader_txn_s", "reader_ops_s",
              "reader_entry_ops_s", "reader_txn_p50_us", "reader_txn_p99_us", "reader_txn_p999_us", "reader_begin_p50_us",
              "reader_begin_p99_us", "reader_stale_mean", "reader_stale_max", "reader_stale_nonzero_fraction",
              "reader_jain", "reader_clock_ghz", "response_p99_ms_upper"]
PERF_METRICS = ["throughput_tx_s", "hitm_loads_per_tx", "hitm_supplied_per_tx", "l2_miss_loads_per_tx",
                "llc_miss_per_tx", "instructions_per_tx", "cycles_per_tx", "ipc", "effective_ghz",
                "ref_busy_cpus"]


def load(root, kind):
    path = root / f"analysis-{kind}" / "summary.json"
    return [r for r in json.loads(path.read_text())["rows"] if not r.get("failure")] if path.exists() else []


def med(values):
    values = [v for v in values if v is not None]
    return statistics.median(values) if values else None


def main(root):
    root = pathlib.Path(root)
    scale, rw, perf = load(root, "scale"), load(root, "rw"), load(root, "perf")
    for r in rw:  # reader_ops_s counts a scan as one op; this counts every entry read
        if r.get("reader_gets_s") is not None:
            r["reader_entry_ops_s"] = r["reader_gets_s"] + r["reader_scan_entries_s"]
    medians = {}

    def put(key, values):
        value = med(values)
        if value is not None:
            medians[key] = value

    groups = collections.defaultdict(list)
    for r in scale:
        groups[(r["cohort"], r["variant"], r["writers"])].append(r)
    for (cohort, variant, w), rows in groups.items():
        for m in SCALE_METRICS:
            put(f"scale|{cohort}|{m}|{variant}|{w}", [r.get(m) for r in rows])
        medians[f"scale|{cohort}|off_target|{variant}|{w}"] = sum(r.get("clock_off_target") is True for r in rows)
        medians[f"scale|{cohort}|cells|{variant}|{w}"] = len(rows)
    groups = collections.defaultdict(list)
    for r in rw:
        groups[(r["variant"], r["writers"], r["readers"])].append(r)
    for (variant, w, rd), rows in groups.items():
        for m in RW_METRICS:
            put(f"rw|{w}|{rd}|{m}|{variant}", [r.get(m) for r in rows])
        medians[f"rw|{w}|{rd}|off_target|{variant}"] = sum(r.get("clock_off_target") is True for r in rows)
        medians[f"rw|{w}|{rd}|cells|{variant}"] = len(rows)
    groups = collections.defaultdict(list)
    for r in perf:
        groups[(r["cohort"], r["variant"], r["writers"], r["readers"])].append(r)
    for (cohort, variant, w, rd), rows in groups.items():
        for m in PERF_METRICS:
            put(f"perf|{cohort}|{w}|{rd}|{m}|{variant}", [r.get(m) for r in rows])
    (OUT / "medians.json").write_text(json.dumps(medians, indent=0, sort_keys=True))

    def line(ax, variant, xs, key):
        pts = [(x, medians.get(key(x))) for x in xs]
        pts = [(x, y) for x, y in pts if y is not None]
        if pts:
            color, style = STYLE[variant]
            ax.plot(*zip(*pts), marker="o", ms=4, color=color, ls=style, label=variant,
                    lw=2.4 if variant == "fc_pq" else 1.3)

    def socket_line(ax):
        ax.axvline(32.5, color="k", ls="-.", lw=1, alpha=0.6)
        ax.text(33, 0.98, "node 1 →", transform=ax.get_xaxis_transform(), fontsize=7, va="top")

    panels = [("all1", "throughput_tx_s", "tx/s: all1", 1e-3, "k tx/s"),
              ("all1", "tx_jain", "tx-count Jain: all1", 1, ""),
              ("all1", "service_jain", "service-time Jain: all1", 1, ""),
              ("half1_half64", "throughput_records_s", "records/s: half1_half64", 1e-3, "k records/s"),
              ("half1_half64", "service_jain", "service-time Jain: half1_half64", 1, ""),
              ("half1_half64", "long_service_share", "64-record clients' service share (fair 0.5)", 1, "")]
    fig, axes = plt.subplots(2, 3, figsize=(16, 9))
    for ax, (cohort, m, title, scale_y, ylabel) in zip(axes.flat, panels):
        for v in VARIANTS:
            pts = [(w, medians.get(f"scale|{cohort}|{m}|{v}|{w}")) for w in SCALE_W]
            pts = [(w, y * scale_y) for w, y in pts if y is not None]
            if pts:
                color, style = STYLE[v]
                ax.plot(*zip(*pts), marker="o", ms=4, color=color, ls=style, label=v, lw=2.4 if v == "fc_pq" else 1.3)
        ax.set_xscale("log", base=2)
        ax.set_xticks(SCALE_W, [str(w) for w in SCALE_W])
        ax.set_title(title, fontsize=10)
        ax.set_xlabel("writer clients (32 = one socket; 64 = two)")
        ax.set_ylabel(ylabel)
        ax.grid(alpha=0.3)
        socket_line(ax)
    axes[0, 0].set_ylim(bottom=0)
    axes[0, 0].legend(fontsize=8)
    fig.tight_layout()
    fig.savefig(OUT / "scale.png", dpi=140)
    plt.close(fig)

    readers = [0, 1, 4, 8, 16]
    fig, axes = plt.subplots(2, 4, figsize=(18, 8.5))
    forward = [("reader_ops_s", "reader ops/s (W=8)", 1e-6, "M ops/s"),
               ("reader_txn_p99_us", "reader txn p99 (W=8)", 1, "µs, bucket upper bound"),
               ("throughput_tx_s", "writer tx/s (W=8)", 1e-3, "k tx/s"),
               ("service_jain", "writer service Jain (W=8, all1)", 1, "")]
    for ax, (m, title, s, yl) in zip(axes[0], forward):
        for v in VARIANTS:
            pts = [(r, medians.get(f"rw|8|{r}|{m}|{v}")) for r in readers]
            pts = [(r, y * s) for r, y in pts if y is not None]
            if pts:
                color, style = STYLE[v]
                ax.plot(*zip(*pts), marker="o", ms=4, color=color, ls=style, label=v, lw=2.4 if v == "fc_pq" else 1.3)
        ax.set_title(title, fontsize=10)
        ax.set_xlabel("reader threads R")
        ax.set_ylabel(yl)
        ax.set_xticks(readers)
        ax.grid(alpha=0.3)
        ax.set_ylim(bottom=0)
    axes[0, 1].set_yscale("log")
    axes[0, 1].set_ylim(bottom=20)
    axes[0, 0].legend(fontsize=7)
    writers = [1, 2, 4, 8]
    reverse = [("reader_ops_s", "reader ops/s (R=8)", 1e-6, "M ops/s"),
               ("reader_txn_p99_us", "reader txn p99 (R=8)", 1, "µs, bucket upper bound"),
               ("throughput_tx_s", "writer tx/s (R=8)", 1e-3, "k tx/s"),
               ("reader_stale_mean", "mean staleness (R=8)", 1, "records behind, sampled")]
    for ax, (m, title, s, yl) in zip(axes[1], reverse):
        for v in VARIANTS:
            pts = [(w, medians.get(f"rw|{w}|8|{m}|{v}")) for w in writers]
            pts = [(w, y * s) for w, y in pts if y is not None]
            if pts:
                color, style = STYLE[v]
                ax.plot(*zip(*pts), marker="o", ms=4, color=color, ls=style, label=v, lw=2.4 if v == "fc_pq" else 1.3)
        ax.set_title(title, fontsize=10)
        ax.set_xlabel("writer clients W")
        ax.set_ylabel(yl)
        ax.set_xticks(writers)
        ax.grid(alpha=0.3)
        ax.set_ylim(bottom=0)
    fig.tight_layout()
    fig.savefig(OUT / "readers.png", dpi=140)
    plt.close(fig)

    fig, axes = plt.subplots(1, 2, figsize=(12, 4))
    for ax, cohort in zip(axes, ["all1", "half1_half64"]):
        for v in VARIANTS:
            line(ax, v, SCALE_W, lambda w, v=v, c=cohort: f"scale|{c}|clock_ghz|{v}|{w}")
        ax.axhline(3.0, color="k", lw=0.8, ls="--")
        ax.set_xscale("log", base=2)
        ax.set_xticks(SCALE_W, [str(w) for w in SCALE_W])
        ax.set_title(f"mean writer-CPU clock, {cohort} (target 3.0 GHz)", fontsize=10)
        ax.set_xlabel("writer clients")
        ax.set_ylabel("GHz")
        ax.grid(alpha=0.3)
        socket_line(ax)
    axes[0].legend(fontsize=7)
    fig.tight_layout()
    fig.savefig(OUT / "clock.png", dpi=140)
    plt.close(fig)


if __name__ == "__main__":
    main(sys.argv[1])
