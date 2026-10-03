#!/usr/bin/env python3
"""async-CFL tables (stdlib only).

usage: summarize_cfl.py RESULTS_DIR GLOB [GLOB ...]
  e.g. summarize_cfl.py results 'cfl-*.json'

Table 1: one markdown row per (contention, workers, balance, variant); every
metric is median [min, max] over repeats (bracket omitted when min == max).
Columns as in summarize_dispatch_pq.py (util = CS / window, o = non-CS lock
time per op, L:H, p99s, burden Jain, worst-worker bystander p99, starved
clients / bystanders), plus ratios of Mops/s and util against the same cell's
medians of `dispatch`, dpq-best and `fcpq-h16-home-c16`. dpq-best is the
fair dispatch-pq variant (`dispatch-pq-home-c256` or `dispatch-pq-remote-c256`)
with the higher median Mops/s in that cell; its name is printed below the
table. Ratios are same-window only if the glob selects one window.

Table 2 (runs with --handoff-stats): per handoff (acquisition by a queue
head, previous CS end -> this CS start) for cfl: release / react / pass
cycles and their sum; how heads got the lock (spin / late / woken shares and
mean react of each); grant -> first poll as head and the share of granted
heads that arrived while the lock was still held (early); scan cycles, the
part after the lock word was cleared (on path), waiters examined; head spin
cycles; parks per handoff; spliced / clamp-promoted shares; wait p50 / p99 /
max in handoffs. The wait histogram's last bucket (WAIT_BUCKETS - 1 = 63) is
open-ended, so a quantile that falls in it is printed as "≥ 63" (a lower
bound); max is exact. dispatch-pq rows show spin + queue + grant->start in
the same "sum" column for comparison.
"""
import glob
import json
import os
import sys
from collections import defaultdict

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from summarize import agg, contention_label, fmt  # noqa: E402
from summarize_dispatch_pq import run_metrics as dpq_metrics  # noqa: E402
from summarize_fcpq_sweep import hist_quantile  # noqa: E402

DPQ_FAIR = ("dispatch-pq-home-c256", "dispatch-pq-remote-c256")
FCPQ = "fcpq-h16-home-c16"
# Last wait-histogram bucket (fc_pq::WAIT_BUCKETS - 1): open-ended.
WAIT_OPEN = 63


def run_metrics(r):
    m = dpq_metrics(r)
    for k in ("sum", "rel", "react", "pass", "spin_sh", "late_sh", "woken_sh",
              "spin_react", "late_react", "woken_react", "g2p", "early",
              "scan", "scan_path", "visited", "head_spin", "parks", "moved",
              "promoted_c", "wait_p50", "wait_p99", "wait_max", "busy"):
        m[k] = None
    c = r.get("cfl")
    t = r["measured_secs"] * r["tsc_hz"]
    if c and c["handoffs"]:
        n = c["handoffs"]
        m["rel"] = c["release_cycles"] / n
        m["react"] = c["react_cycles"] / n
        m["pass"] = c["pass_cycles"] / n
        m["sum"] = m["rel"] + m["react"] + m["pass"]
        for k in ("spin", "late", "woken"):
            a = c["acq_" + k]
            m[k + "_sh"] = a / n
            m[k + "_react"] = c["acq_%s_react" % k] / a if a else None
        if c["grants"]:
            m["g2p"] = c["grant_to_poll_cycles"] / c["grants"]
            m["early"] = c["early_heads"] / c["grants"]
        m["scan"] = c["scan_cycles"] / n
        m["scan_path"] = c["scan_on_path_cycles"] / n
        m["visited"] = c["scan_visited"] / n
        m["head_spin"] = c["spin_cycles"] / n
        m["parks"] = c["parks"] / n
        m["moved"] = c["moves"] / n
        m["promoted_c"] = c["promoted"] / n
        m["wait_p50"] = hist_quantile(c["wait_hist"], 0.50)
        m["wait_p99"] = hist_quantile(c["wait_hist"], 0.99)
        m["wait_max"] = c["max_wait"]
        # Worker time spent spinning or scanning as a head, share of W x window.
        m["busy"] = (c["spin_cycles"] + c["scan_cycles"]) / (t * r["config"]["workers"])
    elif r.get("handoff") and r["handoff"]["handoffs"]:
        h = r["handoff"]
        n = h["handoffs"]
        m["sum"] = (h["release_spin_cycles"] + h["release_queue_cycles"]
                    + h["grant_to_start_cycles"]) / n
    return m


def main():
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    files = sorted({f for p in sys.argv[2:] for f in glob.glob(os.path.join(sys.argv[1], p))})
    if not files:
        print("no files match", sys.argv[2:], file=sys.stderr)
        sys.exit(1)
    cells = defaultdict(list)
    for f in files:
        with open(f) as fh:
            r = json.load(fh)
        cfg = r["config"]
        key = (contention_label(cfg), cfg["workers"], cfg["balance_interval"],
               cfg.get("parallel_mode", "spin"), r["lock"])
        cells[key].append(run_metrics(r))

    def med(key, name):
        a = agg([m[name] for m in cells.get(key, [])])
        return a[0] if a else None

    def dpq_best(cell):
        cands = [(med(cell + (v,), "thr"), v) for v in DPQ_FAIR if cell + (v,) in cells]
        return max(cands)[1] if cands else None

    def ratio(key, name, ref):
        if ref is None:
            return "–"
        num, den = med(key, name), med(key[:4] + (ref,), name)
        return "%.2f" % (num / den) if num is not None and den else "–"

    rank = {"dispatch": 0, "dispatch-pq-home-c256": 1, "dispatch-pq-remote-c256": 2,
            "ces-k64-home": 3, FCPQ: 4}
    order = lambda k: (k[3] != "spin", k[0] != "sus", k[1], k[2], rank.get(k[4], 5), k[4])
    keys = sorted(cells, key=order)
    print("%d runs, %d cells; median [min, max]; latencies µs, cycles in TSC cycles." % (
        len(files), len(cells)))
    print()
    print("| cont | W | B | par | variant | n | Mops/s | ×dispatch | ×dpq-best | ×fcpq-c16 | util "
          "| util ×dispatch | util ×dpq-best | util ×fcpq-c16 | service Jain | L:H ops "
          "| light / heavy p99 | burden Jain | byst p99 | starved c / b | o (cyc/op) |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    bests = {}
    for key in keys:
        runs = cells[key]
        col = lambda name: agg([m[name] for m in runs])
        best = dpq_best(key[:4])
        bests[key[:4]] = best
        print("| %s | %d | %d | %s | %s | %d | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s / %s "
              "| %s | %s | %s / %s | %s |" % (
                  key[0], key[1], key[2], key[3], key[4], len(runs),
                  fmt(col("thr"), 3, 1e-6), ratio(key, "thr", "dispatch"),
                  ratio(key, "thr", best), ratio(key, "thr", FCPQ),
                  fmt(col("util"), 3), ratio(key, "util", "dispatch"),
                  ratio(key, "util", best), ratio(key, "util", FCPQ),
                  fmt(col("jain"), 3), fmt(col("lh"), 2),
                  fmt(col("l_p99"), 1), fmt(col("h_p99"), 1),
                  fmt(col("burden"), 3), fmt(col("byst_p99"), 1),
                  fmt(col("starved_c"), 0), fmt(col("starved_b"), 0),
                  fmt(col("o"), 0)))
    print()
    print("dpq-best per cell: " + ", ".join(
        "%s W%d b%d %s = %s" % (c[0], c[1], c[2], c[3], b) for c, b in sorted(bests.items()) if b))
    inst = [k for k in keys if any(m["sum"] is not None for m in cells[k])]
    if not inst:
        return
    print()
    print("| cont | W | B | par | variant | n | o | release / react / pass = sum (per handoff) "
          "| spin / late / woken share | react spin / late / woken | grant→poll | early "
          "| scan (on path) | visited | head spin | parks | spliced / promoted | busy "
          "| wait p50 / p99 / max |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    def wait_q(runs, name):
        a = agg([m[name] for m in runs])
        if a is None:
            return "–"
        if a[0] >= WAIT_OPEN:
            return "≥ %d" % WAIT_OPEN
        s = fmt(a, 0)
        return s + (" (hi ≥ %d)" % WAIT_OPEN if a[2] >= WAIT_OPEN else "")

    for key in inst:
        runs = cells[key]
        col = lambda name, d=0: fmt(agg([m[name] for m in runs]), d)
        print("| %s | %d | %d | %s | %s | %d | %s | %s / %s / %s = %s | %s / %s / %s | %s / %s / %s "
              "| %s | %s | %s (%s) | %s | %s | %s | %s / %s | %s | %s / %s / %s |" % (
                  key[0], key[1], key[2], key[3], key[4], len(runs), col("o"),
                  col("rel"), col("react"), col("pass"), col("sum"),
                  col("spin_sh", 2), col("late_sh", 2), col("woken_sh", 2),
                  col("spin_react"), col("late_react"), col("woken_react"),
                  col("g2p"), col("early", 2), col("scan"), col("scan_path"),
                  col("visited", 1), col("head_spin"), col("parks", 2),
                  col("moved", 2), col("promoted_c", 3), col("busy", 3),
                  wait_q(runs, "wait_p50"), wait_q(runs, "wait_p99"), col("wait_max")))


if __name__ == "__main__":
    main()
