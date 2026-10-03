#!/usr/bin/env python3
"""Cross-runtime baseline table (stdlib only): tokio-bench runs
(`tokio-*.json`, crate `coro_tokio_baseline`) against coro-bench runs from the
same time window (`xrt-*.json`).

usage: summarize_tokio.py [RESULTS_DIR]      (default: results/)

One markdown table, one row per (cell, variant); cell = (workers,
contention). Every metric is median [min, max] over repeats (bracket omitted
when min == max). Latencies in µs (cycles / tsc_hz). "thr / tokio-mutex" is
the ratio of throughput medians within the same cell. A class with no ops in
a run contributes no latency sample.
"""
import glob
import json
import os
import statistics
import sys
from collections import defaultdict

TOKIO_ORDER = ["tokio-mutex", "tokio-mutex-unconstrained", "async-lock", "std-mutex", "parking-lot"]
CORO_ORDER = ["dispatch", "dispatch-home", "ces-k64-home", "fc-remote", "fcpq-h16-home"]
REFERENCE = "tokio-mutex"


def contention_label(cfg):
    if cfg["clients"] == 64 and cfg["parallel_work_cycles"] == 4 * cfg["light_cs_cycles"]:
        return "sus"
    if cfg["clients"] == 16 and cfg["parallel_work_cycles"] == 32 * cfg["light_cs_cycles"]:
        return "bur"
    return "c%d-p%d" % (cfg["clients"], cfg["parallel_work_cycles"])


def class_latency_us(r, cls, q):
    lat = r["classes"][cls]["run_latency"]
    if lat["count"] == 0:
        return None
    return lat[q] / r["tsc_hz"] * 1e6


def run_metrics(r):
    return {
        "throughput": r["throughput_ops_per_s"],
        "service_jain": r["service_jain"],
        "light_p50": class_latency_us(r, "light", "p50"),
        "light_p99": class_latency_us(r, "light", "p99"),
        "heavy_p50": class_latency_us(r, "heavy", "p50"),
        "heavy_p99": class_latency_us(r, "heavy", "p99"),
        "starved_clients": r["starved_clients"],
        "t": int(r["timestamp"].split(":")[1]),
    }


def agg(values):
    vals = [v for v in values if v is not None]
    if not vals:
        return None
    return (statistics.median(vals), min(vals), max(vals), len(vals))


def fmt(a, digits=2, scale=1.0):
    if a is None:
        return "–"
    med, lo, hi, n = a
    f = "%%.%df" % digits
    s = f % (med * scale)
    if n > 1 and lo != hi:
        s += " [" + (f % (lo * scale)) + ", " + (f % (hi * scale)) + "]"
    return s


def main():
    results_dir = sys.argv[1] if len(sys.argv) > 1 else "results"
    cells = defaultdict(list)
    window = {}
    for runtime, pattern in (("tokio", "tokio-*.json"), ("coro", "xrt-*.json")):
        files = sorted(glob.glob(os.path.join(results_dir, pattern)))
        if not files:
            print("no files match", os.path.join(results_dir, pattern), file=sys.stderr)
            sys.exit(1)
        stamps = []
        for f in files:
            with open(f) as fh:
                r = json.load(fh)
            cfg = r["config"]
            m = run_metrics(r)
            stamps.append(m["t"])
            key = (cfg["workers"], contention_label(cfg), runtime, r["lock"])
            cells[key].append(m)
        window[runtime] = (len(files), min(stamps), max(stamps))

    order = {v: i for i, v in enumerate(TOKIO_ORDER + CORO_ORDER)}
    cont_order = {"sus": 0, "bur": 1}
    keys = sorted(cells, key=lambda k: (k[0], cont_order.get(k[1], 9), k[2] != "tokio",
                                        order.get(k[3], 99), k[3]))

    print("tokio-bench: %d runs, unix %d–%d; coro-bench: %d runs, unix %d–%d. "
          "median [min, max] over repeats; latencies in µs." % (
              window["tokio"] + window["coro"]))
    print()
    print("| workers | contention | runtime | variant | n | throughput (Mops/s) | thr / %s | "
          "service Jain | light run p50 / p99 (µs) | heavy run p50 / p99 (µs) | starved clients |"
          % REFERENCE)
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    for k in keys:
        w, cont, runtime, variant = k
        runs = cells[k]
        col = lambda name: agg([m[name] for m in runs])
        thr = col("throughput")
        ref = agg([m["throughput"] for m in cells.get((w, cont, "tokio", REFERENCE), [])])
        ratio = "%.2f" % (thr[0] / ref[0]) if thr and ref else "–"
        print("| %d | %s | %s | %s | %d | %s | %s | %s | %s / %s | %s / %s | %s |" % (
            w, cont, runtime, variant, len(runs),
            fmt(thr, 3, 1e-6), ratio,
            fmt(col("service_jain"), 3),
            fmt(col("light_p50"), 1), fmt(col("light_p99"), 1),
            fmt(col("heavy_p50"), 1), fmt(col("heavy_p99"), 1),
            fmt(col("starved_clients"), 0)))


if __name__ == "__main__":
    main()
