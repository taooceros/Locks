#!/usr/bin/env python3
"""FC-PQ starvation-clamp / newcomer-init sweep table (stdlib only).

usage: summarize_fcpq_sweep.py RESULTS_DIR GLOB [GLOB ...]
  e.g. summarize_fcpq_sweep.py results 'p3-fcpq-h16-home*-w8-h8-b31-*.json' \\
           'p3-fc-remote-w8-h8-b31-*.json'

One markdown row per (contention, workers, balance, variant); every metric is
median [min, max] over repeats (bracket omitted when min == max).

Columns (T = measured_secs x tsc_hz, the window in TSC cycles; CS = harness
service cycles, i.e. insert + spin measured inside the closure):
  util         CS / T: the fraction of the window the lock spent executing
               critical sections (the lock is one serial resource, so the
               denominator is T, not W x T); same definition as the
               2026-09-28 phase-3 "lock utilisation" column.
  busy         total_combining_cycles / T: fraction of the window some
               combiner was inside a pass (includes the drain after the
               window, < 0.1 %).
  admin/op     (total_combining_cycles - CS) / ops: in-pass cycles per op not
               spent in a critical section (drain, heap, wakes, rekey).
  gap/op       (T - total_combining_cycles) / ops: cycles per op with no
               combiner active (hand-off between combiners).
  L:H          light ops / heavy ops.
  CS/op L, H   service cycles per op by class.
  wait         fcpq runs with --fcpq-wait-stats only (`fcpq_wait` in the
               JSON): p50 / p99 / max passes between admission and service,
               and the fraction of served requests the starvation clamp had
               promoted.
"""
import glob
import json
import os
import sys
from collections import defaultdict

sys.dont_write_bytecode = True  # importing summarize.py must not leave __pycache__/
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from summarize import agg, contention_label, fmt, us  # noqa: E402


def hist_quantile(hist, q):
    """Smallest wait w with P(wait <= w) >= q; the last bucket is open."""
    total = sum(hist)
    if total == 0:
        return None
    target = q * total
    acc = 0
    for w, n in enumerate(hist):
        acc += n
        if acc >= target:
            return w
    return len(hist) - 1


def run_metrics(r):
    hz = r["tsc_hz"]
    t = r["measured_secs"] * hz
    lc, hc = r["classes"]["light"], r["classes"]["heavy"]
    cs = lc["service_cycles"] + hc["service_cycles"]
    ops = r["total_ops"]
    comb = r["total_combining_cycles"]
    m = {
        "thr": r["throughput_ops_per_s"],
        "jain": r["service_jain"],
        "util": cs / t,
        "busy": comb / t if comb else None,
        "admin": (comb - cs) / ops if comb and ops else None,
        "gap": (t - comb) / ops if comb and ops else None,
        "lh": lc["ops"] / hc["ops"] if hc["ops"] else None,
        "cs_l": lc["service_cycles"] / lc["ops"] if lc["ops"] else None,
        "cs_h": hc["service_cycles"] / hc["ops"] if hc["ops"] else None,
        "l_p99": us(lc["run_latency"]["p99"], hz),
        "h_p50": us(hc["run_latency"]["p50"], hz),
        "h_p99": us(hc["run_latency"]["p99"], hz),
        "h_max": us(hc["run_latency"]["max"], hz),
        "starved": r["starved_clients"],
        "w_p50": None,
        "w_p99": None,
        "w_max": None,
        "promoted": None,
        "ops_pass": None,
    }
    w = r.get("fcpq_wait")
    if w and w["served"]:
        m["w_p50"] = hist_quantile(w["wait_hist"], 0.50)
        m["w_p99"] = hist_quantile(w["wait_hist"], 0.99)
        m["w_max"] = w["max_wait"]
        m["promoted"] = w["promoted"] / w["served"]
        m["ops_pass"] = w["served"] / w["passes"] if w["passes"] else None
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

    print("%d runs, %d cells; median [min, max]; latencies µs, cycles in TSC cycles." % (
        len(files), len(cells)))
    print()
    print("| cont | W | B | variant | n | thr (Mops/s) | service Jain | util | busy | admin/op | gap/op "
          "| L:H ops | CS/op L / H | light p99 | heavy p50 / p99 / max | wait p50 / p99 / max (passes) "
          "| clamp-promoted | ops/pass | starved |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    order = lambda k: (k[0] != "sus", k[1], k[2], k[3])
    for key in sorted(cells, key=order):
        runs = cells[key]
        col = lambda name: agg([m[name] for m in runs])
        print("| %s | %d | %d | %s | %d | %s | %s | %s | %s | %s | %s | %s | %s / %s | %s | %s / %s / %s "
              "| %s / %s / %s | %s | %s | %s |" % (
                  key[0], key[1], key[2], key[3], len(runs),
                  fmt(col("thr"), 3, 1e-6), fmt(col("jain"), 3),
                  fmt(col("util"), 3), fmt(col("busy"), 3),
                  fmt(col("admin"), 0), fmt(col("gap"), 0),
                  fmt(col("lh"), 2), fmt(col("cs_l"), 0), fmt(col("cs_h"), 0),
                  fmt(col("l_p99"), 1),
                  fmt(col("h_p50"), 1), fmt(col("h_p99"), 1), fmt(col("h_max"), 0),
                  fmt(col("w_p50"), 0), fmt(col("w_p99"), 0), fmt(col("w_max"), 0),
                  fmt(col("promoted"), 3), fmt(col("ops_pass"), 1),
                  fmt(col("starved"), 0)))


if __name__ == "__main__":
    main()
