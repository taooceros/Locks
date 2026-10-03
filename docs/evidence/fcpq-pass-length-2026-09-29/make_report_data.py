"""medians.json, tables.md and summary.png for report.typ from the three roots' analyses.

Usage (from the workspace root):
    python3 docs/evidence/fcpq-pass-length-2026-09-29/make_report_data.py \
        .worktree/fcpqh-timed-01 .worktree/fcpqh-stats-01 .worktree/fcpqh-perf-01

  timed  primary binary, upstream_gate/fc/fc_pq/fc_pq_hn/fc_pq_h8, 1/2/4/8 clients  (analysis/summary.json)
  stats  combiner_pass_stat binary, the four delegation variants                    (analysis/summary.json)
  perf   primary binary under perf stat, 8 clients                                  (analysis-perf/summary.json)

Key format of medians.json: "metric|cohort|variant|clients" (durability None throughout).
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
VARIANTS = ["upstream_gate", "fc", "fc_pq", "fc_pq_hn", "fc_pq_h8"]
DELEGATED = ["fc", "fc_pq", "fc_pq_hn", "fc_pq_h8"]
COHORTS = ["all1", "half1_half64"]
CLIENTS = [1, 2, 4, 8]
TIMED = ["throughput_tx_s", "throughput_records_s", "service_jain", "tx_jain", "long_service_share",
         "response_p50_ms", "response_p99_ms", "worker_p99_ms_max", "short_response_p99_ms",
         "long_response_p99_ms", "served_remote_fraction", "max_worker_served_share",
         "max_worker_executed_share", "own_local_fraction",
         "fast_path_hit_rate", "service_utilization", "clock_ghz"]
STATS = ["bodies_per_pass", "bodies_weighted_pass_length", "combiner_changes_per_1k_bodies",
         "combiner_changes_nonempty_per_1k_bodies", "empty_pass_fraction", "max_bodies_per_pass",
         "throughput_tx_s", "max_worker_served_share", "service_jain"]
PERF = ["hitm_loads_per_tx", "hitm_supplied_per_tx", "l2_miss_loads_per_tx", "l2_miss_all_per_tx",
        "llc_miss_per_tx", "ipc", "instructions_per_tx", "cycles_per_tx", "throughput_tx_s", "clock_ghz"]


def load(root, analysis):
    data = json.load(open(pathlib.Path(root) / analysis / "summary.json"))
    return data, [r for r in data["rows"] if not r.get("failure")]


def collect(rows, metrics, prefix=""):
    groups = collections.defaultdict(list)
    for r in rows:
        for m in metrics:
            if r.get(m) is not None:
                groups[(prefix + m, r["cohort"], r["variant"], r["clients"])].append(r[m])
    return groups


def main(timed_root, stats_root, perf_root, out=None):
    global OUT
    if out:  # e.g. round1/: the replicate round's tables next to the main ones
        OUT = pathlib.Path(out)
        OUT.mkdir(parents=True, exist_ok=True)
    timed, timed_rows = load(timed_root, "analysis")
    stats, stats_rows = load(stats_root, "analysis")
    perf, perf_rows = load(perf_root, "analysis-perf")
    groups = {}
    groups.update(collect(timed_rows, TIMED))
    groups.update(collect(stats_rows, STATS, "stats_"))
    groups.update(collect(perf_rows, PERF, "perf_"))
    medians = {"|".join(map(str, k)): statistics.median(v) for k, v in groups.items()}
    ranges = {"|".join(map(str, k)): [min(v), max(v), len(v)] for k, v in groups.items()}
    (OUT / "medians.json").write_text(json.dumps(medians, indent=0, sort_keys=True))
    (OUT / "ranges.json").write_text(json.dumps(ranges, indent=0, sort_keys=True))

    def m(metric, cohort, variant, clients):
        return medians.get(f"{metric}|{cohort}|{variant}|{clients}")

    def cell(metric, cohort, variant, clients, digits=2, scale=1.0):
        key = f"{metric}|{cohort}|{variant}|{clients}"
        if key not in ranges:
            return "—"
        lo, hi, _ = ranges[key]
        mid = medians[key]
        return f"{mid * scale:.{digits}f} [{lo * scale:.{digits}f}, {hi * scale:.{digits}f}]"

    lines = []
    for cohort in COHORTS:
        lines += [f"### {cohort}, 8 clients, durability None (median [min, max] of 3)", "",
                  "| variant | tx/s | records/s | HITM loads/tx | L2 misses/tx | LLC misses/tx | IPC | bodies/pass | "
                  "body-weighted pass | combiner changes /1k | service Jain | p99 ms (all) | p99 ms (worst worker) |",
                  "|" + "---|" * 13]
        for v in DELEGATED + ["upstream_gate"]:
            lines.append(
                f"| {v} | {cell('throughput_tx_s', cohort, v, 8, 0)} | {cell('throughput_records_s', cohort, v, 8, 0)} | "
                f"{cell('perf_hitm_loads_per_tx', cohort, v, 8, 1)} | {cell('perf_l2_miss_all_per_tx', cohort, v, 8, 1)} | "
                f"{cell('perf_llc_miss_per_tx', cohort, v, 8, 2)} | {cell('perf_ipc', cohort, v, 8, 2)} | "
                f"{cell('stats_bodies_per_pass', cohort, v, 8, 2)} | "
                f"{cell('stats_bodies_weighted_pass_length', cohort, v, 8, 2)} | "
                f"{cell('stats_combiner_changes_per_1k_bodies', cohort, v, 8, 1)} | "
                f"{cell('service_jain', cohort, v, 8, 3)} | {cell('response_p99_ms', cohort, v, 8, 3)} | "
                f"{cell('worker_p99_ms_max', cohort, v, 8, 3)} |")
        lines.append("")
    for metric, label, digits in (("throughput_tx_s", "tx/s", 0), ("service_jain", "service Jain", 3),
                                  ("response_p99_ms", "p99 response ms (merged)", 3),
                                  ("own_local_fraction", "fraction of requests run on their own requester's thread", 3),
                                  ("max_worker_executed_share", "largest single-worker share of all bodies (combiner concentration)", 3),
                                  ("stats_bodies_per_pass", "bodies per pass (stats build)", 2),
                                  ("stats_combiner_changes_per_1k_bodies", "combiner changes per 1k bodies (stats build)", 1),
                                  ("stats_throughput_tx_s", "tx/s of the stats build", 0)):
        for cohort in COHORTS:
            lines += [f"### {label}: {cohort}", "", "| variant | " + " | ".join(f"{c} clients" for c in CLIENTS) + " |",
                      "|" + "---|" * (len(CLIENTS) + 1)]
            for v in VARIANTS:
                row = [cell(metric, cohort, v, c, digits) for c in CLIENTS]
                if any(x != "—" for x in row):
                    lines.append(f"| {v} | " + " | ".join(row) + " |")
            lines.append("")
    # Per-worker (8 clients): response p50/p99 (bucket upper bounds, median over repeats) and the
    # requests served while the worker was combiner, as a share of all bodies.
    lines += ["## Per-worker response time and combiner service, 8 clients (timed root)", ""]
    for cohort in COHORTS:
        for v in DELEGATED + ["upstream_gate"]:
            chosen = [r for r in timed_rows if (r["cohort"], r["variant"], r["clients"]) == (cohort, v, 8)]
            if not chosen:
                continue
            n = len(chosen[0]["worker_p99_ms"])
            sizes = ["1"] * (n // 2) + ["64"] * (n // 2) if cohort == "half1_half64" else ["1"] * n
            lines += [f"### {cohort}, {v}", "",
                      "| worker (records/req) | " + " | ".join(f"w{i} ({sizes[i]})" for i in range(n)) + " |",
                      "|" + "---|" * (n + 1)]
            for label, key, scale, digits in (("p50 ms", "worker_p50_ms", 1, 3), ("p99 ms", "worker_p99_ms", 1, 3)):
                lines.append(f"| {label} | " + " | ".join(
                    "—" if any(r[key][i] is None for r in chosen)  # no request completed in the window
                    else f"{statistics.median(r[key][i] for r in chosen):.{digits}f}" for i in range(n)) + " |")
            if chosen[0].get("worker_executed_bodies"):
                for label, key in (("bodies run while combiner (share of all; own included)", "worker_executed_bodies"),
                                   ("... of which for other workers", "worker_served_for_others")):
                    shares = [[r[key][i] / sum(r["worker_executed_bodies"]) for i in range(n)] for r in chosen]
                    lines.append(f"| {label} | " + " | ".join(
                        f"{statistics.median(s[i] for s in shares):.3f}" for i in range(n)) + " |")
            lines.append("")
    lines += ["## Derived (ratios of medians)", "",
              "### Stats build tx/s over primary-build tx/s (counter overhead), all client counts", "",
              "| cohort | variant | " + " | ".join(f"{c} clients" for c in CLIENTS) + " |",
              "|" + "---|" * (len(CLIENTS) + 2)]
    for cohort in COHORTS:
        for v in DELEGATED:
            ratios = []
            for c in CLIENTS:
                a, b = m("stats_throughput_tx_s", cohort, v, c), m("throughput_tx_s", cohort, v, c)
                ratios.append(f"{a / b:.3f}" if a and b else "—")
            lines.append(f"| {cohort} | {v} | " + " | ".join(ratios) + " |")
    lines += ["", "### tx/s against fc and fc_pq (8 clients) and share of the fc_pq - fc gap retained", "",
              "| cohort | variant | tx/s / fc | tx/s / fc_pq | (variant - fc) / (fc_pq - fc) |", "|---|---|---|---|---|"]
    for cohort in COHORTS:
        fc, pq = m("throughput_tx_s", cohort, "fc", 8), m("throughput_tx_s", cohort, "fc_pq", 8)
        for v in ("fc_pq", "fc_pq_hn", "fc_pq_h8"):
            x = m("throughput_tx_s", cohort, v, 8)
            lines.append(f"| {cohort} | {v} | {x / fc:.3f} | {x / pq:.3f} | {(x - fc) / (pq - fc):.2f} |")
    lines.append("")
    clock = {"timed": timed.get("clock"), "stats": stats.get("clock"), "perf": perf.get("clock")}
    lines += ["### Clock (power setup S1, 3.0 GHz)", "", "```json", json.dumps(clock, indent=1), "```", ""]
    (OUT / "tables.md").write_text("\n".join(lines))

    fig, axes = plt.subplots(2, 3, figsize=(16, 9))
    styles = {"upstream_gate": "k:", "fc": "C0-", "fc_pq": "C3-", "fc_pq_hn": "C2--", "fc_pq_h8": "C1--"}
    panels = [
        ("throughput_tx_s", "all1", "tx/s, all1"),
        ("throughput_tx_s", "half1_half64", "tx/s, half1_half64"),
        ("service_jain", "half1_half64", "service-time Jain, half1_half64"),
        ("stats_bodies_per_pass", "all1", "bodies per combining pass, all1 (stats build)"),
        ("stats_combiner_changes_per_1k_bodies", "all1", "combiner changes per 1k bodies, all1"),
    ]
    hitm_ax = axes[1, 2]
    for ax, (metric, cohort, title) in zip(axes.flat, panels):
        for v in VARIANTS:
            pts = [(c, m(metric, cohort, v, c)) for c in CLIENTS]
            pts = [(c, y) for c, y in pts if y is not None]
            if len(pts) > 1:
                ax.plot(*zip(*pts), styles[v], marker="o", label=v, lw=2.2 if v == "fc_pq" else 1.3)
            elif pts:
                ax.plot(*pts[0], marker="o", ls="none", color=styles[v][:2], label=v)
        ax.set_title(title, fontsize=10)
        ax.set_xticks(CLIENTS)
        ax.set_xlabel("clients")
        ax.grid(alpha=0.3)
    axes[0, 0].legend(fontsize=8)
    width = 0.2
    for k, v in enumerate(DELEGATED):
        hitm_ax.bar([i + (k - 1.5) * width for i in range(len(COHORTS))],
                    [m("perf_hitm_loads_per_tx", cohort, v, 8) or 0 for cohort in COHORTS], width,
                    label=v, color=styles[v][:2])
    hitm_ax.set_xticks(range(len(COHORTS)), COHORTS)
    hitm_ax.set_title("HITM loads per tx, 8 clients (perf cohort)", fontsize=10)
    hitm_ax.grid(alpha=0.3, axis="y")
    fig.tight_layout()
    fig.savefig(OUT / "summary.png", dpi=140)
    print("wrote medians.json, ranges.json, tables.md, summary.png")


if __name__ == "__main__":
    main(*sys.argv[1:5])
