#!/usr/bin/env python3
"""Compact markdown tables for the `co2-*` same-window matrix (stdlib only).

usage: summarize_co2.py RESULTS_DIR

Reads co2-<label>-w<W>-h8-b31-<sus|bur>-<spin|yield>-r<i>.json (coro-bench,
scripts/run_co2.sh) and co2-tokio-mutex-w8-h8-<sus|bur>-<spin|yield>-r<i>.json
(tokio-bench, scripts/run_co2_tokio.sh). Every value is median [min, max] over
the repeats of one cell (bracket omitted when min == max). Metrics are those of
summarize_co.py: `o` = (window - sum CS cycles) / ops; burden = foreign-CS Jain
(ces, co-*), combining-cycles Jain (fc, fcpq), `-` otherwise; bystander p99 =
worst worker; starved = clients / bystanders with no progress.
"""
import glob
import json
import os
import sys
from collections import defaultdict

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import summarize_co as sc  # noqa: E402
from summarize import agg, contention_label, fmt, us  # noqa: E402

ORDER = ["co-pq-sremote-k64-c256", "co-pq-sremote-k64-c0", "co-fifo-sremote-k64-home", "ces-k64-home",
         "fc-remote", "fcpq-h16-home-c16", "dispatch", "dispatch-pq-home-c256"]


def tokio_metrics(r):
    hz = r["tsc_hz"]
    t = r["measured_secs"] * hz
    lc, hc = r["classes"]["light"], r["classes"]["heavy"]
    cs = lc["service_cycles"] + hc["service_cycles"]
    return {"thr": r["throughput_ops_per_s"], "jain": r["service_jain"], "o": (t - cs) / r["total_ops"],
            "lh": lc["ops"] / hc["ops"], "l_p99": us(lc["run_latency"]["p99"], hz),
            "h_p99": us(hc["run_latency"]["p99"], hz), "burden": None,
            "byst_p99": us(r["bystander_latency"]["p99"], hz), "starved_c": r["starved_clients"],
            "starved_b": r.get("starved_bystanders")}


def main():
    res = sys.argv[1]
    cells = defaultdict(list)
    for f in sorted(glob.glob(os.path.join(res, "co2-*.json"))):
        with open(f) as fh:
            r = json.load(fh)
        cfg = r["config"]
        tokio = os.path.basename(f).startswith("co2-tokio-")
        key = (contention_label(cfg), cfg["workers"], cfg["parallel_mode"], r["lock"])
        cells[key].append(tokio_metrics(r) if tokio else sc.run_metrics(r))
    print("| cell | par | lock | n | Mops/s | svc. Jain | L:H | o | light / heavy p99 (µs) | burden J "
          "| byst p99 (µs) | starved c/b |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|")

    def rank(k):
        return (k[0] != "sus", k[1], k[2] != "spin", ORDER.index(k[3]) if k[3] in ORDER else 99, k[3])

    for k in sorted(cells, key=rank):
        runs = cells[k]
        c = lambda n: agg([m[n] for m in runs])
        print("| W%d %s | %s | `%s` | %d | %s | %s | %s | %s | %s / %s | %s | %s | %s / %s |" % (
            k[1], k[0], k[2], k[3], len(runs), fmt(c("thr"), 3, 1e-6), fmt(c("jain"), 3), fmt(c("lh"), 2),
            fmt(c("o"), 0), fmt(c("l_p99"), 0), fmt(c("h_p99"), 0), fmt(c("burden"), 3), fmt(c("byst_p99"), 1),
            fmt(c("starved_c"), 0), fmt(c("starved_b"), 0)))


if __name__ == "__main__":
    main()
