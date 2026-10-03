#!/usr/bin/env python3
"""Aggregate coro-bench JSON results into markdown tables (stdlib only).

usage: summarize.py [RESULTS_DIR] [GLOB]      (defaults: results/  matrix-*.json)

Cells are (contention, workers, heavy_ratio, balance_interval, lock); every
metric is reported as median [min, max] over repeats. Latencies are in
microseconds (cycles / tsc_hz). "combiner worker" = worker with the largest
combining_cycles in that run; "non-combiner p99" = max bystander p99 over the
other workers; "pass" = H (64) x mean critical-section cycles of the run.
"""
import glob
import json
import os
import statistics
import sys
from collections import defaultdict

H_PASS = 64


def contention_label(cfg):
    if cfg["clients"] == 64 and cfg["parallel_work_cycles"] == 4 * cfg["light_cs_cycles"]:
        return "sus"
    if cfg["clients"] == 16 and cfg["parallel_work_cycles"] == 32 * cfg["light_cs_cycles"]:
        return "bur"
    return "c%d-p%d" % (cfg["clients"], cfg["parallel_work_cycles"])


def us(cycles, hz):
    return cycles / hz * 1e6


def run_metrics(r):
    hz = r["tsc_hz"]
    workers = r["workers"]
    total_comb = sum(w["combining_cycles"] for w in workers)
    m = {
        "throughput": r["throughput_ops_per_s"],
        "service_jain": r["service_jain"],
        "burden_jain": r["burden_jain"],
        "starved_clients": r["starved_clients"],
        "starved_bystanders": r.get("starved_bystanders"),
        "light_p50": us(r["classes"]["light"]["run_latency"]["p50"], hz),
        "light_p99": us(r["classes"]["light"]["run_latency"]["p99"], hz),
        "heavy_p50": us(r["classes"]["heavy"]["run_latency"]["p50"], hz),
        "heavy_p99": us(r["classes"]["heavy"]["run_latency"]["p99"], hz),
        "combiner_share": None,
        "combiner_p99": None,
        "noncomb_p99": None,
        "pass_us": None,
        "light_ops": r["classes"]["light"]["ops"],
        "heavy_ops": r["classes"]["heavy"]["ops"],
        "yields_per_op": (r.get("total_combiner_yields", 0) / r["total_ops"]) if r["total_ops"] else None,
        "chain_p50": (r.get("chain_lengths") or {}).get("p50"),
        "chain_max": (r.get("chain_lengths") or {}).get("max"),
        "chain_n": (r.get("chain_lengths") or {}).get("count"),
        "pl_inline": (r.get("placements") or {}).get("inline"),
        "pl_remote": (r.get("placements") or {}).get("remote"),
        "pl_home": (r.get("placements") or {}).get("home"),
    }
    if r["total_ops"]:
        svc = sum(c["service_cycles"] for c in r["clients"])
        m["pass_us"] = us(H_PASS * svc / r["total_ops"], hz)
    if total_comb > 0:
        comb = max(workers, key=lambda w: w["combining_cycles"])
        m["combiner_share"] = comb["combining_cycles"] / total_comb
        if comb["bystander_latency"]["count"] > 0:
            m["combiner_p99"] = us(comb["bystander_latency"]["p99"], hz)
        others = [
            us(w["bystander_latency"]["p99"], hz)
            for w in workers
            if w is not comb and w["bystander_latency"]["count"] > 0
        ]
        if others:
            m["noncomb_p99"] = max(others)
    else:
        # no combiner: report the max bystander p99 over all workers as "non-combiner"
        others = [
            us(w["bystander_latency"]["p99"], hz)
            for w in workers
            if w["bystander_latency"]["count"] > 0
        ]
        if others:
            m["noncomb_p99"] = max(others)
    return m


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
    if n > 1 and (lo != hi):
        s += " [" + (f % (lo * scale)) + ", " + (f % (hi * scale)) + "]"
    return s


def main():
    results_dir = sys.argv[1] if len(sys.argv) > 1 else "results"
    pattern = sys.argv[2] if len(sys.argv) > 2 else "matrix-*.json"
    files = sorted(glob.glob(os.path.join(results_dir, pattern)))
    if not files:
        print("no files match", os.path.join(results_dir, pattern), file=sys.stderr)
        sys.exit(1)

    cells = defaultdict(list)
    for f in files:
        with open(f) as fh:
            r = json.load(fh)
        cfg = r["config"]
        key = (contention_label(cfg), cfg["workers"], cfg["heavy_ratio"],
               cfg["balance_interval"], r["lock"])
        cells[key].append(run_metrics(r))

    lock_order = {"dispatch": 0, "ces": 1, "fc": 2, "fcpq": 3, "fc-noyield": 4, "fcpq-noyield": 5}
    combiner_locks = ["ces", "fc", "fcpq", "fc-noyield", "fcpq-noyield"]
    groups = sorted({k[:4] for k in cells}, key=lambda g: (g[0] != "sus", g[1], g[2], g[3]))

    print("# coro-bench summary")
    print()
    print("%d runs, %d cells. median [min, max] over repeats; latencies in µs." % (
        len(files), len(cells)))
    print()
    for g in groups:
        cont, w, h, b = g
        print("## %s, workers=%d, heavy_ratio=%d, balance_interval=%d" % (
            {"sus": "sustained (64 clients, 4x parallel)",
             "bur": "bursty (16 clients, 32x parallel)"}.get(cont, cont), w, h, b))
        print()
        print("| lock | n | throughput (Mops/s) | service Jain | burden Jain | combiner share | "
              "combiner bystander p99 | non-combiner p99 (max) | pass = 64×mean CS | "
              "light run p50 / p99 | heavy run p50 / p99 | starved clients | starved bystanders | "
              "chains n / p50 / max | yields/op | placements inline / remote / home |")
        print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
        locks = sorted({k[4] for k in cells if k[:4] == g},
                       key=lambda l: (lock_order.get(l.split("-")[0], 9), l))
        for lock in locks:
            runs = cells[g + (lock,)]
            col = lambda name: agg([m[name] for m in runs])
            print("| %s | %d | %s | %s | %s | %s | %s | %s | %s | %s / %s | %s / %s | %s | %s | %s / %s / %s | %s | %s / %s / %s |" % (
                lock, len(runs),
                fmt(col("throughput"), 3, 1e-6),
                fmt(col("service_jain"), 3),
                fmt(col("burden_jain"), 3),
                fmt(col("combiner_share"), 2),
                fmt(col("combiner_p99"), 1),
                fmt(col("noncomb_p99"), 1),
                fmt(col("pass_us"), 1),
                fmt(col("light_p50"), 1), fmt(col("light_p99"), 1),
                fmt(col("heavy_p50"), 1), fmt(col("heavy_p99"), 1),
                fmt(col("starved_clients"), 0),
                fmt(col("starved_bystanders"), 0),
                fmt(col("chain_n"), 0), fmt(col("chain_p50"), 0), fmt(col("chain_max"), 0),
                fmt(col("yields_per_op"), 2),
                fmt(col("pl_inline"), 0), fmt(col("pl_remote"), 0), fmt(col("pl_home"), 0),
            ))
        print()

    # Hypothesis helpers -----------------------------------------------------
    print("## Hypothesis helpers")
    print()
    print("### H-A: burden Jain (sustained), by lock")
    print()
    print("| workers | heavy | balance | " + " | ".join(combiner_locks) + " |")
    print("|---|---|---|" + "---|" * len(combiner_locks))
    for g in groups:
        if g[0] != "sus":
            continue
        row = []
        for lock in combiner_locks:
            runs = cells.get(g + (lock,), [])
            row.append(fmt(agg([m["burden_jain"] for m in runs]), 3))
        print("| %d | %d | %d | %s |" % (g[1], g[2], g[3], " | ".join(row)))
    print()
    print("### H-B (bursty): combiner bystander p99 − non-combiner p99 vs one pass, µs")
    print()
    print("| workers | heavy | balance | lock | combiner p99 | non-combiner p99 | difference | pass | verdict |")
    print("|---|---|---|---|---|---|---|---|---|")
    for g in groups:
        if g[0] != "bur":
            continue
        for lock in combiner_locks:
            runs = cells.get(g + (lock,), [])
            if not runs:
                continue
            diffs = [m["combiner_p99"] - m["noncomb_p99"] for m in runs
                     if m["combiner_p99"] is not None and m["noncomb_p99"] is not None]
            a_c = agg([m["combiner_p99"] for m in runs])
            a_n = agg([m["noncomb_p99"] for m in runs])
            a_d = agg(diffs)
            a_p = agg([m["pass_us"] for m in runs])
            verdict = "–"
            if a_d and a_p:
                verdict = "exceeds pass" if a_d[1] > a_p[2] else (
                    "below pass" if a_d[2] < a_p[1] else "within spread")
            print("| %d | %d | %d | %s | %s | %s | %s | %s | %s |" % (
                g[1], g[2], g[3], lock, fmt(a_c, 1), fmt(a_n, 1), fmt(a_d, 1), fmt(a_p, 1), verdict))
    print()
    print("### H-C (heavy_ratio=8): service Jain and fcpq throughput relative to fc / ces")
    print()
    print("| contention | workers | balance | dispatch J | ces J | fc J | fcpq J | fc-noyield J | fcpq-noyield J | "
          "fcpq light/heavy ops | fcpq-noyield light/heavy ops | fcpq/fc thr | fcpq-noyield/fc-noyield thr | fcpq/ces thr | verdict (fcpq vs FIFO) |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    all_locks = ["dispatch", "ces", "fc", "fcpq", "fc-noyield", "fcpq-noyield"]
    for g in groups:
        if g[2] != 8:
            continue
        js = {}
        thr = {}
        lh = {}
        for lock in all_locks:
            runs = cells.get(g + (lock,), [])
            js[lock] = agg([m["service_jain"] for m in runs])
            thr[lock] = agg([m["throughput"] for m in runs])
            lh[lock] = agg([m["light_ops"] / m["heavy_ops"] for m in runs if m["heavy_ops"]])

        def ratio(a, b):
            return "%.2f" % (thr[a][0] / thr[b][0]) if thr.get(a) and thr.get(b) else "–"

        verdict = "–"
        if js.get("fcpq") and js.get("fc") and thr.get("fcpq") and thr.get("fc"):
            fifo = [x[0] for k, x in js.items() if k in ("dispatch", "ces", "fc", "fc-noyield") and x]
            ok_j = js["fcpq"][1] >= 0.95 and max(fifo) <= 0.90
            r = thr["fcpq"][0] / thr["fc"][0]
            verdict = ("holds" if ok_j and r >= 0.9 else
                       "fairness ok, tax > 10%" if ok_j else "fairness fails")
        print("| %s | %d | %d | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s |" % (
            g[0], g[1], g[3],
            fmt(js["dispatch"], 3), fmt(js["ces"], 3), fmt(js["fc"], 3), fmt(js["fcpq"], 3),
            fmt(js["fc-noyield"], 3), fmt(js["fcpq-noyield"], 3),
            fmt(lh["fcpq"], 2), fmt(lh["fcpq-noyield"], 2),
            ratio("fcpq", "fc"), ratio("fcpq-noyield", "fc-noyield"), ratio("fcpq", "ces"),
            verdict))
    print()
    print("### fcpq tax vs fc at heavy_ratio=1 (throughput ratio, medians)")
    print()
    print("| contention | workers | balance | fc Mops/s | fcpq Mops/s | fcpq/fc | fc-noyield Mops/s | fcpq-noyield Mops/s | fcpq-noyield/fc-noyield |")
    print("|---|---|---|---|---|---|---|---|---|")
    for g in groups:
        if g[2] != 1:
            continue
        a = {lock: agg([m["throughput"] for m in cells.get(g + (lock,), [])])
             for lock in ["fc", "fcpq", "fc-noyield", "fcpq-noyield"]}
        if not (a["fc"] and a["fcpq"]):
            continue
        r_ny = ("%.2f" % (a["fcpq-noyield"][0] / a["fc-noyield"][0])
                if a["fc-noyield"] and a["fcpq-noyield"] else "–")
        print("| %s | %d | %d | %s | %s | %.2f | %s | %s | %s |" % (
            g[0], g[1], g[3], fmt(a["fc"], 3, 1e-6), fmt(a["fcpq"], 3, 1e-6),
            a["fcpq"][0] / a["fc"][0], fmt(a["fc-noyield"], 3, 1e-6),
            fmt(a["fcpq-noyield"], 3, 1e-6), r_ny))

    # Phase 3: mitigations and placement -------------------------------------
    print()
    print("## Phase 3: mitigations (H-D) and placement, per (contention, workers, heavy, balance)")
    print()
    print("Throughput ratios use the same cell's `ces` and `dispatch` medians as reference.")
    print()
    for g in groups:
        locks = sorted({k[4] for k in cells if k[:4] == g},
                       key=lambda l: (lock_order.get(l.split("-")[0], 9), l))
        if len(locks) < 3:
            continue
        ref_ces = agg([m["throughput"] for m in cells.get(g + ("ces",), [])])
        ref_dis = agg([m["throughput"] for m in cells.get(g + ("dispatch",), [])])
        print("### %s, workers=%d, heavy=%d, balance=%d" % (g[0], g[1], g[2], g[3]))
        print()
        print("| variant | burden Jain | combiner share | combiner bystander p99 | non-combiner p99 | "
              "starved bystanders | starved clients | service Jain | light/heavy ops | throughput (Mops/s) | "
              "thr/ces | thr/dispatch | chains p50 / max | yields/op |")
        print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
        for lock in locks:
            runs = cells[g + (lock,)]
            col = lambda name: agg([m[name] for m in runs])
            thr = col("throughput")
            lh = agg([m["light_ops"] / m["heavy_ops"] for m in runs if m["heavy_ops"]])
            print("| %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s / %s | %s |" % (
                lock,
                fmt(col("burden_jain"), 3), fmt(col("combiner_share"), 2),
                fmt(col("combiner_p99"), 1), fmt(col("noncomb_p99"), 1),
                fmt(col("starved_bystanders"), 0), fmt(col("starved_clients"), 0),
                fmt(col("service_jain"), 3), fmt(lh, 2),
                fmt(thr, 3, 1e-6),
                "%.2f" % (thr[0] / ref_ces[0]) if thr and ref_ces else "–",
                "%.2f" % (thr[0] / ref_dis[0]) if thr and ref_dis else "–",
                fmt(col("chain_p50"), 0), fmt(col("chain_max"), 0),
                fmt(col("yields_per_op"), 2)))
        print()


if __name__ == "__main__":
    main()
