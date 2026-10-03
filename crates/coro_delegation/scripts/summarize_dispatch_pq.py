#!/usr/bin/env python3
"""dispatch-pq kill-test tables (stdlib only).

usage: summarize_dispatch_pq.py RESULTS_DIR GLOB [GLOB ...]
  e.g. summarize_dispatch_pq.py results 'dpq-*.json'

One markdown row per (contention, workers, balance, variant); every metric is
median [min, max] over repeats (bracket omitted when min == max). Ratios use
the medians of the same cell's `dispatch` and `fcpq-h16-home-c16` rows, so
they are same-window only if the glob selects one window.

Columns (T = measured_secs x tsc_hz, the window in TSC cycles; CS = harness
service cycles, i.e. insert + spin measured inside the closure):
  util        CS / T: fraction of the window the lock spent executing
              critical sections (one serial resource).
  o           (T - CS) / ops: non-CS lock time per op. For a lock that is
              always held or in handoff this is the per-op lock cost (fcpq:
              in-pass admin + hand-off gap; dispatch / dispatch-pq: release
              path + wake-to-critical-section latency).
  L:H         light ops / heavy ops.
  byst p99    worst worker's bystander schedule->poll p99.
  starved     clients with 0 ops / bystander samples censored at half the
              window.
Second table (runs with --handoff-stats / --fcpq-wait-stats only):
  fast/free/handoff  share of acquisitions by path (dispatch-pq).
  spin, queue, grant->start  cycles per handoff (HandoffStats).
  wait        queue wait p50 / p99 / max in ticks (handoffs for
              dispatch-pq, passes for fcpq) and the clamp-promoted share.
"""
import glob
import json
import os
import sys
from collections import defaultdict

sys.dont_write_bytecode = True  # importing summarize.py must not leave __pycache__/
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from summarize import agg, contention_label, fmt, us  # noqa: E402
from summarize_fcpq_sweep import hist_quantile  # noqa: E402

REFS = ("dispatch", "fcpq-h16-home-c16")


def run_metrics(r):
    hz = r["tsc_hz"]
    t = r["measured_secs"] * hz
    lc, hc = r["classes"]["light"], r["classes"]["heavy"]
    cs = lc["service_cycles"] + hc["service_cycles"]
    ops = r["total_ops"]
    byst = [us(w["bystander_latency"]["p99"], hz) for w in r["workers"]
            if w["bystander_latency"]["count"] > 0]
    m = {
        "thr": r["throughput_ops_per_s"],
        "jain": r["service_jain"],
        "burden": r["burden_jain"],
        "util": cs / t,
        "o": (t - cs) / ops if ops else None,
        "lh": lc["ops"] / hc["ops"] if hc["ops"] else None,
        "l_p99": us(lc["run_latency"]["p99"], hz),
        "h_p99": us(hc["run_latency"]["p99"], hz),
        "byst_p99": max(byst) if byst else None,
        "starved_c": r["starved_clients"],
        "starved_b": r.get("starved_bystanders"),
        "fast": None, "free": None, "hand": None,
        "spin": None, "queue": None, "g2s": None,
        "w_p50": None, "w_p99": None, "w_max": None, "promoted": None,
    }
    h = r.get("handoff")
    if h:
        acq = h["fast"] + h["free"] + h["handoffs"]
        if acq:
            m["fast"], m["free"], m["hand"] = (h["fast"] / acq, h["free"] / acq,
                                               h["handoffs"] / acq)
        if h["handoffs"]:
            n = h["handoffs"]
            m["spin"] = h["release_spin_cycles"] / n
            m["queue"] = h["release_queue_cycles"] / n
            m["g2s"] = h["grant_to_start_cycles"] / n
    w = r.get("fcpq_wait")
    if w and w["served"]:
        m["w_p50"] = hist_quantile(w["wait_hist"], 0.50)
        m["w_p99"] = hist_quantile(w["wait_hist"], 0.99)
        m["w_max"] = w["max_wait"]
        m["promoted"] = w["promoted"] / w["served"]
    return m


def main():
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    results_dir = sys.argv[1]
    files = sorted({f for p in sys.argv[2:] for f in glob.glob(os.path.join(results_dir, p))})
    if not files:
        print("no files match", sys.argv[2:], file=sys.stderr)
        sys.exit(1)
    cells = defaultdict(list)
    for f in files:
        with open(f) as fh:
            r = json.load(fh)
        cfg = r["config"]
        key = (contention_label(cfg), cfg["workers"], cfg["balance_interval"], r["lock"])
        cells[key].append(run_metrics(r))

    def med(key, name):
        a = agg([m[name] for m in cells.get(key, [])])
        return a[0] if a else None

    def ratio(key, name, ref):
        num, den = med(key, name), med(key[:3] + (ref,), name)
        return "%.2f" % (num / den) if num is not None and den else "–"

    order = lambda k: (k[0] != "sus", k[1], k[2],
                       {"dispatch": 0, "fc-remote": 2, "fcpq-h16-home-c16": 3}.get(k[3], 1), k[3])
    keys = sorted(cells, key=order)
    print("%d runs, %d cells; median [min, max]; latencies µs, cycles in TSC cycles." % (
        len(files), len(cells)))
    print()
    print("| cont | W | B | variant | n | Mops/s | ×dispatch | ×fcpq-c16 | util | util ×dispatch "
          "| util ×fcpq-c16 | service Jain | L:H ops | light / heavy p99 | burden Jain "
          "| byst p99 | starved c / b | o (cyc/op) |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    for key in keys:
        runs = cells[key]
        col = lambda name: agg([m[name] for m in runs])
        print("| %s | %d | %d | %s | %d | %s | %s | %s | %s | %s | %s | %s | %s | %s / %s | %s "
              "| %s | %s / %s | %s |" % (
                  key[0], key[1], key[2], key[3], len(runs),
                  fmt(col("thr"), 3, 1e-6), ratio(key, "thr", REFS[0]), ratio(key, "thr", REFS[1]),
                  fmt(col("util"), 3), ratio(key, "util", REFS[0]), ratio(key, "util", REFS[1]),
                  fmt(col("jain"), 3), fmt(col("lh"), 2),
                  fmt(col("l_p99"), 1), fmt(col("h_p99"), 1),
                  fmt(col("burden"), 3), fmt(col("byst_p99"), 1),
                  fmt(col("starved_c"), 0), fmt(col("starved_b"), 0),
                  fmt(col("o"), 0)))
    inst = [k for k in keys if any(m["hand"] is not None or m["w_p50"] is not None
                                   for m in cells[k])]
    if not inst:
        return
    print()
    print("| cont | W | B | variant | n | fast / free / handoff | spin / queue / grant→start "
          "(cyc per handoff) | wait p50 / p99 / max (ticks) | clamp-promoted |")
    print("|---|---|---|---|---|---|---|---|---|")
    for key in inst:
        runs = cells[key]
        col = lambda name: agg([m[name] for m in runs])
        print("| %s | %d | %d | %s | %d | %s / %s / %s | %s / %s / %s | %s / %s / %s | %s |" % (
            key[0], key[1], key[2], key[3], len(runs),
            fmt(col("fast"), 3), fmt(col("free"), 3), fmt(col("hand"), 3),
            fmt(col("spin"), 0), fmt(col("queue"), 0), fmt(col("g2s"), 0),
            fmt(col("w_p50"), 0), fmt(col("w_p99"), 0), fmt(col("w_max"), 0),
            fmt(col("promoted"), 3)))


if __name__ == "__main__":
    main()
