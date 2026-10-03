#!/usr/bin/env python3
"""Same-window matrix tables (FINDINGS 2026-09-30 "same-window matrix:
ordinary-waker co-pq, spin vs yield"), coro-bench and tokio-bench JSONs
(stdlib only).

usage: summarize_pw1.py RESULTS_DIR GLOB [GLOB ...]
  e.g. summarize_pw1.py results 'pw1-*.json'

Cells are (contention, W, parallel mode); every metric is median [min, max]
over repeats (bracket omitted when min == max). Ratios use medians of the
same cell, so they are same-window only if the glob selects one window.

Columns (T = measured_secs x tsc_hz, the window in TSC cycles; CS = harness
service cycles, insert + spin measured inside the critical section):
  Mops/s      total ops / s (counts cheap light ops like heavy ones).
  Jain        service Jain over per-client CS cycles.
  o           (T - CS) / ops: cycles per op in which the serial resource ran
              no critical section (lock cost + idle lock).
  util        CS / T: share of the window spent inside critical sections.
              Of each CS, the TSC spin (1000 / 8000 cycles) is fixed, so a
              lock can move util only through o and the served mix.
  C̄           CS / ops: mean CS cycles per op of the mix the lock served.
  L:H         light ops / heavy ops.
  p99 L / H   run-latency p99 per class, µs (6 % histogram buckets, lower
              bounds). co-* include the step-aside of unlock().await.
  max wait    worst queue wait in lock ticks (`fcpq_wait.max_wait`: passes
              for fcpq, hand-offs for dispatch-pq and co-pq); '–' where
              not instrumented.
  max run     worst single run latency over both classes, µs (histogram
              bucket lower bound; every lock, tokio included).
  burden      foreign-CS Jain (`burden_foreign_jain`, REVIEW I8) for ces /
              co-*; fc / fcpq: combining-cycle Jain (pass definition, not
              the same quantity), marked †; '–' for locks that never run
              another task's CS (dispatch, dispatch-pq, cfl) and for tokio.
  byst p99    worst worker's bystander schedule->poll p99, µs; tokio: the
              merged p99, measured by the task (includes tokio's defer
              list; REVIEW I5), marked ‡.
  starved     clients with 0 ops / bystander samples censored at half the
              window.

The o-by-mix table (REVIEW I3) compares o only between locks that served
the same mix in the same cell: "FIFO mix" = L:H < 1.5, "fair mix" = L:H
>= 4. It is derived from window totals of each run; o at a *fixed* mix other
than the one a lock served is not derivable from these data (one equation,
two per-class unknowns) and is not printed.
"""
import glob
import json
import os
import statistics
import sys
from collections import defaultdict

sys.dont_write_bytecode = True

FOREIGN = ("ces", "co-fifo", "co-pq")
COMBINING = ("fc", "fcpq", "actor")


def contention_label(cfg):
    if cfg["clients"] == 64 and cfg["parallel_work_cycles"] == 4 * cfg["light_cs_cycles"]:
        return "sus"
    if cfg["clients"] == 16 and cfg["parallel_work_cycles"] == 32 * cfg["light_cs_cycles"]:
        return "bur"
    return "c%d-p%d" % (cfg["clients"], cfg["parallel_work_cycles"])


def us(cycles, hz):
    return cycles / hz * 1e6


def label_of(r):
    if r.get("runtime") == "tokio":
        return "tokio:" + r["lock"]
    return r["lock"]


def run_metrics(r):
    hz = r["tsc_hz"]
    t = r["measured_secs"] * hz
    lc, hc = r["classes"]["light"], r["classes"]["heavy"]
    cs = lc["service_cycles"] + hc["service_cycles"]
    ops = r["total_ops"]
    lock = r["lock"]
    tokio = r.get("runtime") == "tokio"
    if tokio:
        byst = us(r["bystander_latency"]["p99"], hz)
    else:
        b = [us(w["bystander_latency"]["p99"], hz) for w in r["workers"]
             if w["bystander_latency"]["count"] > 0]
        byst = max(b) if b else None
    burden, burden_kind = None, ""
    if not tokio:
        base = lock.split("-")[0]
        if lock.startswith(FOREIGN):
            burden, burden_kind = r.get("burden_foreign_jain"), "foreign"
        elif base in COMBINING:
            burden, burden_kind = r.get("burden_jain"), "combining"
    w = r.get("fcpq_wait")
    return {
        "thr": r["throughput_ops_per_s"],
        "jain": r["service_jain"],
        "o": (t - cs) / ops if ops else None,
        "util": cs / t,
        "cbar": cs / ops if ops else None,
        "lh": lc["ops"] / hc["ops"] if hc["ops"] else None,
        "l_p99": us(lc["run_latency"]["p99"], hz),
        "h_p99": us(hc["run_latency"]["p99"], hz),
        "max_run": max(us(lc["run_latency"]["max"], hz), us(hc["run_latency"]["max"], hz)),
        "w_max": w["max_wait"] if w and w.get("served") else None,
        "burden": burden,
        "burden_kind": burden_kind,
        "byst_p99": byst,
        "starved_c": r["starved_clients"],
        "starved_b": r.get("starved_bystanders"),
        "sync": (r.get("co_mutex") or {}).get("sync_drops"),
        "timestamp": r.get("timestamp"),
    }


def load_cells(results_dir, patterns):
    """{(cont, W, par, label): [run_metrics, ...]} and the file count."""
    files = sorted({f for p in patterns for f in glob.glob(os.path.join(results_dir, p))})
    cells = defaultdict(list)
    for f in files:
        with open(f) as fh:
            r = json.load(fh)
        cfg = r["config"]
        key = (contention_label(cfg), cfg["workers"], cfg.get("parallel_mode", "spin"), label_of(r))
        cells[key].append(run_metrics(r))
    return cells, len(files)


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
    if n > 1 and (f % (lo * scale)) != (f % (hi * scale)):
        s += " [" + (f % (lo * scale)) + ", " + (f % (hi * scale)) + "]"
    return s


def med(runs, name):
    a = agg([m[name] for m in runs])
    return a[0] if a else None


ORDER = ["co-pq-sremote-k64-home-c256", "co-pq-snone-k64-home-c256",
         "co-fifo-sremote-k64-home", "co-fifo-snone-k64-home",
         "dispatch", "dispatch-pq-home-c256", "dispatch-pq-remote-c256", "cfl",
         "ces-k64-home", "fc-remote", "fcpq-h16-home-c16",
         "tokio:tokio-mutex", "tokio:async-lock"]


def order_key(k):
    cont, w, par, lab = k
    return (cont != "sus", w, par != "spin", ORDER.index(lab) if lab in ORDER else 99, lab)


def mix_group(lh):
    if lh is None:
        return None
    if lh < 1.5:
        return "FIFO"
    if lh >= 4:
        return "fair"
    return None


def main():
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    cells, n = load_cells(sys.argv[1], sys.argv[2:])
    if not cells:
        print("no files match", sys.argv[2:], file=sys.stderr)
        sys.exit(1)
    keys = sorted(cells, key=order_key)
    stamps = sorted(m["timestamp"] for runs in cells.values() for m in runs if m["timestamp"])
    print("%d runs, %d cells, first %s, last %s; median [min, max]; latencies µs, "
          "cycles TSC." % (n, len(cells), stamps[0] if stamps else "?", stamps[-1] if stamps else "?"))
    print()
    print("| cont | W | par | variant | n | Mops/s | Jain | o (cyc/op) | util | C̄ | L:H "
          "| p99 L / H | max wait | max run | burden | byst p99 | starved c / b |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    for key in keys:
        runs = cells[key]
        col = lambda name: agg([m[name] for m in runs])
        kind = runs[0]["burden_kind"]
        burden = fmt(col("burden"), 3) + ("†" if kind == "combining" and col("burden") else "")
        byst = fmt(col("byst_p99"), 1) + ("‡" if key[3].startswith("tokio:") else "")
        print("| %s | %d | %s | %s | %d | %s | %s | %s | %s | %s | %s | %s / %s | %s | %s | %s | %s "
              "| %s / %s |" % (
                  key[0], key[1], key[2], key[3], len(runs),
                  fmt(col("thr"), 3, 1e-6), fmt(col("jain"), 3), fmt(col("o"), 0),
                  fmt(col("util"), 3), fmt(col("cbar"), 0), fmt(col("lh"), 2),
                  fmt(col("l_p99"), 0), fmt(col("h_p99"), 0), fmt(col("w_max"), 0),
                  fmt(col("max_run"), 0), burden, byst,
                  fmt(col("starved_c"), 0), fmt(col("starved_b"), 0)))

    # spin vs yield per lock
    print()
    print("Yield / spin, medians of the same window:")
    print()
    print("| cont | W | variant | Mops/s spin | Mops/s yield | yield/spin | o spin | o yield |")
    print("|---|---|---|---|---|---|---|---|")
    for key in keys:
        if key[2] != "spin":
            continue
        yk = key[:2] + ("yield",) + key[3:]
        if yk not in cells:
            continue
        s, y = cells[key], cells[yk]
        print("| %s | %d | %s | %.3f | %.3f | %.2f | %.0f | %.0f |" % (
            key[0], key[1], key[3], med(s, "thr") * 1e-6, med(y, "thr") * 1e-6,
            med(y, "thr") / med(s, "thr"), med(s, "o"), med(y, "o")))

    # o by served mix (REVIEW I3)
    print()
    print("o by served mix (same cell, same mix only; ratio to the group "
          "reference fc-remote (FIFO) or fcpq-h16-home-c16 (fair), else the "
          "group's lowest o):")
    print()
    print("| cont | W | par | mix | variant | L:H | C̄ | o | o / ref | util |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    by_cell = defaultdict(list)
    for key in keys:
        by_cell[key[:3]].append(key)
    for cell, ks in by_cell.items():
        groups = defaultdict(list)
        for k in ks:
            g = mix_group(med(cells[k], "lh"))
            if g:
                groups[g].append(k)
        for g in ("FIFO", "fair"):
            if g not in groups:
                continue
            ref_lab = "fc-remote" if g == "FIFO" else "fcpq-h16-home-c16"
            refk = next((k for k in groups[g] if k[3] == ref_lab), None)
            if refk is None:
                refk = min(groups[g], key=lambda k: med(cells[k], "o"))
            ref_o = med(cells[refk], "o")
            for k in groups[g]:
                runs = cells[k]
                col = lambda name: agg([m[name] for m in runs])
                print("| %s | %d | %s | %s | %s | %s | %s | %s | %.2f | %s |" % (
                    cell[0], cell[1], cell[2], g, k[3], fmt(col("lh"), 2), fmt(col("cbar"), 0),
                    fmt(col("o"), 0), med(runs, "o") / ref_o, fmt(col("util"), 3)))


if __name__ == "__main__":
    main()
