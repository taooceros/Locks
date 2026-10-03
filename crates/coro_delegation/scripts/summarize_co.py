#!/usr/bin/env python3
"""Coroutine-style mutex (co-fifo / co-pq) tables against the references
(stdlib only).

usage: summarize_co.py RESULTS_DIR GLOB [GLOB ...]
  e.g. summarize_co.py results 'co1-*.json'

One markdown row per (contention, workers, balance, parallel mode, variant);
every metric is median [min, max] over repeats (bracket omitted when
min == max). Ratios use the medians of the same cell's reference rows, so
they are same-window only if the glob selects one window.

Columns (T = measured_secs x tsc_hz, the window in TSC cycles; CS = harness
service cycles, i.e. insert + spin measured inside the critical section):
  o           (T - CS) / ops: per-op lock cost (non-CS time of the serial
              resource per op). Mix-independent only if the gap per op does
              not depend on the class of the ops around it.
  thr@FIFO    T_1s / (C_fifo + o), C_fifo = (mean light CS + mean heavy CS)/2
              of the same run: the throughput this run's o would give at the
              FIFO (1:1) mix if o were class-independent. A prediction, not
              a measurement.
  L:H         light ops / heavy ops.
  burden      Jain over per-worker foreign critical-section cycles
              (`burden_foreign_jain`, stats::record_foreign_cs) for ces and
              co-*; fc/fcpq: Jain over per-worker combining cycles
              (`burden_jain`, pass definition); '-' for the rest.
  byst p99    worst worker's bystander schedule->poll p99.
  starved     clients with 0 ops / bystander samples censored at half the
              window.
  step/op     co-*: unlock() calls that stepped aside, per acquisition;
              breaks = chain-break handoffs per acquisition; sync = guard
              drops (all counted over each client's window).
Burden detail (third table): foreign share = foreign CS cycles / all CS
cycles; foreign / combining Jain as above; client-poll Jain and max share =
Jain and largest per-worker share of client poll cycles (where client tasks,
CS and parallel work together, actually ran).
"""
import glob
import json
import os
import sys
from collections import defaultdict

sys.dont_write_bytecode = True  # importing summarize.py must not leave __pycache__/
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from summarize import agg, contention_label, fmt, us  # noqa: E402

REFS = ("ces-k64-home", "fc-remote", "fcpq-h16-home-c16")
FOREIGN = ("ces", "co-fifo", "co-pq")
COMBINING = ("fc", "fcpq", "actor")


def run_metrics(r):
    hz = r["tsc_hz"]
    t = r["measured_secs"] * hz
    lc, hc = r["classes"]["light"], r["classes"]["heavy"]
    cs = lc["service_cycles"] + hc["service_cycles"]
    ops = r["total_ops"]
    lock = r["lock"]
    byst = [us(w["bystander_latency"]["p99"], hz) for w in r["workers"]
            if w["bystander_latency"]["count"] > 0]
    o = (t - cs) / ops if ops else None
    thr_fifo = None
    if o is not None and lc["ops"] and hc["ops"]:
        c_fifo = (lc["service_cycles"] / lc["ops"] + hc["service_cycles"] / hc["ops"]) / 2
        thr_fifo = hz / (c_fifo + o)
    base = lock.split("-")[0]
    if lock.startswith(FOREIGN):
        burden = r.get("burden_foreign_jain")
    elif base in COMBINING:
        burden = r["burden_jain"]
    else:
        burden = None
    m = {
        "thr": r["throughput_ops_per_s"],
        "thr_fifo": thr_fifo,
        "jain": r["service_jain"],
        "o": o,
        "util": cs / t,
        "lh": lc["ops"] / hc["ops"] if hc["ops"] else None,
        "l_p99": us(lc["run_latency"]["p99"], hz),
        "h_p99": us(hc["run_latency"]["p99"], hz),
        "burden": burden,
        "byst_p99": max(byst) if byst else None,
        "starved_c": r["starved_clients"],
        "starved_b": r.get("starved_bystanders"),
        "step": None, "breaks": None, "sync": None, "fast": None,
        "w_max": None,
        "foreign_share": r["total_foreign_cs_cycles"] / cs if cs else None,
        "foreign_jain": r.get("burden_foreign_jain"),
        "comb_jain": r["burden_jain"],
        "poll_jain": jain([w["client_poll_cycles"] for w in r["workers"]]),
        "poll_max": (max(w["client_poll_cycles"] for w in r["workers"])
                     / max(1, sum(w["client_poll_cycles"] for w in r["workers"]))),
    }
    co = r.get("co_mutex")
    if co:
        acq = co["fast"] + co["free"] + co["handoff"]
        if acq:
            m["step"] = co["step_asides"] / acq
            m["breaks"] = co["chain_breaks"] / acq
            m["fast"] = co["fast"] / acq
        m["sync"] = co["sync_drops"]
    w = r.get("fcpq_wait")
    if w and w["served"]:
        m["w_max"] = w["max_wait"]
    return m


def jain(xs):
    s, ss = sum(xs), sum(x * x for x in xs)
    return s * s / (len(xs) * ss) if ss else None


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
        key = (contention_label(cfg), cfg["workers"], cfg["balance_interval"],
               cfg.get("parallel_mode", "spin"), r["lock"])
        cells[key].append(run_metrics(r))

    def med(key, name):
        a = agg([m[name] for m in cells.get(key, [])])
        return a[0] if a else None

    def ratio(key, name, ref):
        num, den = med(key, name), med(key[:4] + (ref,), name)
        return "%.2f" % (num / den) if num is not None and den else "–"

    rank = {"dispatch": 0, "ces-k64-home": 1, "fc-remote": 2, "fcpq-h16-home-c16": 3}
    order = lambda k: (k[0] != "sus", k[1], k[2], k[3] != "spin",
                       rank.get(k[4], 4 if k[4].startswith("dispatch-pq") else 5), k[4])
    keys = sorted(cells, key=order)
    print("%d runs, %d cells; median [min, max]; latencies µs, cycles in TSC cycles." % (
        len(files), len(cells)))
    print()
    print("| cont | W | B | par | variant | n | Mops/s | ×ces-k64 | ×fc-remote | ×fcpq-c16 "
          "| service Jain | o (cyc/op) | thr@FIFO mix (Mops/s) | L:H | light / heavy p99 "
          "| burden Jain | byst p99 | starved c / b |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    for key in keys:
        runs = cells[key]
        col = lambda name: agg([m[name] for m in runs])
        print("| %s | %d | %d | %s | %s | %d | %s | %s | %s | %s | %s | %s | %s | %s | %s / %s "
              "| %s | %s | %s / %s |" % (
                  key[0], key[1], key[2], key[3], key[4], len(runs),
                  fmt(col("thr"), 3, 1e-6), ratio(key, "thr", REFS[0]),
                  ratio(key, "thr", REFS[1]), ratio(key, "thr", REFS[2]),
                  fmt(col("jain"), 3), fmt(col("o"), 0), fmt(col("thr_fifo"), 3, 1e-6),
                  fmt(col("lh"), 2), fmt(col("l_p99"), 0), fmt(col("h_p99"), 0),
                  fmt(col("burden"), 3), fmt(col("byst_p99"), 1),
                  fmt(col("starved_c"), 0), fmt(col("starved_b"), 0)))
    co_keys = [k for k in keys if any(m["step"] is not None for m in cells[k])]
    if co_keys:
        print()
        print("| cont | W | B | par | variant | n | fast share | step-asides / op "
              "| chain breaks / op | sync drops | max wait (handoffs) |")
        print("|---|---|---|---|---|---|---|---|---|---|---|")
        for key in co_keys:
            runs = cells[key]
            col = lambda name: agg([m[name] for m in runs])
            print("| %s | %d | %d | %s | %s | %d | %s | %s | %s | %s | %s |" % (
                key[0], key[1], key[2], key[3], key[4], len(runs),
                fmt(col("fast"), 3), fmt(col("step"), 3), fmt(col("breaks"), 4),
                fmt(col("sync"), 0), fmt(col("w_max"), 0)))
    print()
    print("| cont | W | B | par | variant | n | foreign CS share | foreign Jain | combining Jain "
          "| client-poll Jain | client-poll max share |")
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    for key in keys:
        runs = cells[key]
        col = lambda name: agg([m[name] for m in runs])
        print("| %s | %d | %d | %s | %s | %d | %s | %s | %s | %s | %s |" % (
            key[0], key[1], key[2], key[3], key[4], len(runs),
            fmt(col("foreign_share"), 3), fmt(col("foreign_jain"), 3),
            fmt(col("comb_jain"), 3), fmt(col("poll_jain"), 3), fmt(col("poll_max"), 3)))


if __name__ == "__main__":
    main()
