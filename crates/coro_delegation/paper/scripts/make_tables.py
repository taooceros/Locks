#!/usr/bin/env python3
"""Generate the paper's LaTeX table includes from the result JSONs (stdlib only).

usage: make_tables.py [RESULTS_DIR] [OUT_DIR]     (defaults: ../results  tables)

Every table cell is median [min, max] over the repeats of one configuration
(bracket omitted when min == max), computed with the metric definitions of
`crates/coro_delegation/scripts/summarize.py` (throughput, Jain indices,
bystander p99s, combiner worker) and `scripts/summarize_fcpq_sweep.py` (util,
admin/op, gap/op, queue waits), which are imported, not re-derived.

Measurement windows: runs whose JSON timestamp is before 2026-09-29 00:07:54
UTC (unix 1790640474) ran at turbo clock ("09-28"); later runs ran with CPUs
0-15 capped at 3.0 GHz ("09-29"). Each table states which window its rows
come from; the script refuses to aggregate a cell that mixes windows.

Also writes `numbers.tex`: \\newcommand macros for the derived numbers the
text quotes (model Jain, required light:heavy ratio, required per-op cost).
The LIFO side check is read from `results/xrt-side-lifo.tar.zst` (Python
>= 3.14 for tarfile's zstd support).
"""
import glob
import json
import os
import re
import statistics
import sys
import tarfile
from collections import defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.join(HERE, "..", "..", "scripts"))
import summarize  # noqa: E402
import summarize_fcpq_sweep as sweep  # noqa: E402

CAP_UNIX = 1790640474  # 2026-09-29 00:07:54 UTC: 3.0 GHz cap set on CPUs 0-15
CENSORED_US = 1e6       # a bystander p99 above 1 s is a censored sample


# ---------------------------------------------------------------- loading --

def window_of(r):
    return "09-29" if int(r["timestamp"].split(":")[1]) >= CAP_UNIX else "09-28"


def metrics(r):
    """All per-run metrics used by the tables; None where not applicable."""
    m = {"window": window_of(r)}
    hz = r["tsc_hz"]
    lc, hc = r["classes"]["light"], r["classes"]["heavy"]
    t = r["measured_secs"] * hz
    cs = lc["service_cycles"] + hc["service_cycles"]
    ops = r["total_ops"]
    m.update({
        "thr": r["throughput_ops_per_s"],
        "sj": r["service_jain"],
        "starved_c": r["starved_clients"],
        "starved_b": r.get("starved_bystanders"),
        "lh": lc["ops"] / hc["ops"] if hc["ops"] else None,
        "cs_l": lc["service_cycles"] / lc["ops"] if lc["ops"] else None,
        "cs_h": hc["service_cycles"] / hc["ops"] if hc["ops"] else None,
        "util": cs / t,
        # per-op cost not spent in a critical section; for combining locks
        # this is admin/op + gap/op of summarize_fcpq_sweep.py exactly.
        "o": (t - cs) / ops if ops else None,
        "cbar": cs / ops if ops else None,
        "h_p50": summarize.us(hc["run_latency"]["p50"], hz) if hc["run_latency"]["count"] else None,
        "h_p99": summarize.us(hc["run_latency"]["p99"], hz) if hc["run_latency"]["count"] else None,
        "l_p99": summarize.us(lc["run_latency"]["p99"], hz) if lc["run_latency"]["count"] else None,
        "bj": None, "comb_p99": None, "noncomb_p99": None, "share": None,
        "admin": None, "gap": None, "w_p99": None, "w_max": None, "promoted": None,
        "by_p99": None,
    })
    if "workers" in r and isinstance(r["workers"], list):  # coro-bench
        s = summarize.run_metrics(r)
        m.update({"bj": s["burden_jain"], "comb_p99": s["combiner_p99"],
                  "noncomb_p99": s["noncomb_p99"], "share": s["combiner_share"]})
        w = sweep.run_metrics(r)
        m.update({"admin": w["admin"], "gap": w["gap"], "w_p99": w["w_p99"],
                  "w_max": w["w_max"], "promoted": w["promoted"]})
    else:  # tokio-bench
        m["by_p99"] = summarize.us(r["bystander_latency"]["p99"], hz)
    return m


def load(results, pattern, key=None):
    """{(cont, W, B, lock): [metrics]} for files matching `pattern`."""
    cells = defaultdict(list)
    files = sorted(glob.glob(os.path.join(results, pattern)))
    if not files:
        sys.exit("make_tables: no files match %s" % os.path.join(results, pattern))
    for f in files:
        with open(f) as fh:
            r = json.load(fh)
        cfg = r["config"]
        k = (summarize.contention_label(cfg), cfg["workers"], cfg.get("balance_interval"), r["lock"])
        cells[k].append(metrics(r))
    for k, runs in cells.items():
        ws = {m["window"] for m in runs}
        if len(ws) != 1:
            sys.exit("make_tables: cell %s in %s mixes windows %s" % (k, pattern, ws))
    return cells


def load_lifo(results):
    """LIFO-slot side check: {(W, cont, lock, 'on'|'off'): [metrics]}."""
    path = os.path.join(results, "xrt-side-lifo.tar.zst")
    cells = defaultdict(list)
    with tarfile.open(path, "r:zst") as tar:
        for mem in tar.getmembers():
            mm = re.match(r"xrt-side-lifo/out2/lifo(on|off)-(.+)-w(\d+)-(sus|bur)-r\d\.json$", mem.name)
            if not mm:
                continue
            r = json.load(tar.extractfile(mem))
            cells[(int(mm.group(3)), mm.group(4), mm.group(2), mm.group(1))].append(metrics(r))
    if not cells:
        sys.exit("make_tables: no LIFO side-check runs in %s" % path)
    return cells


# ------------------------------------------------------------- formatting --

def agg(runs, name):
    return summarize.agg([m[name] for m in runs]) if runs else None


def med(runs, name):
    a = agg(runs, name)
    return a[0] if a else None


def f(a, digits=3, scale=1.0, rng=True):
    """median\\rng{min}{max}; '--' if missing."""
    if a is None:
        return "--"
    md, lo, hi, n = a
    fmt = "%%.%df" % digits
    s = fmt % (md * scale)
    if rng and n > 1 and (fmt % (lo * scale)) != (fmt % (hi * scale)):
        s += "\\rng{%s}{%s}" % (fmt % (lo * scale), fmt % (hi * scale))
    return s


def fus(a, digits=1, rng=True):
    """Latency in µs; censored samples (> 1 s) shown as `cens.`."""
    if a is None:
        return "--"
    if a[0] > CENSORED_US:
        return "cens."
    return f(a, digits, rng=rng)


def fint(a, rng=True):
    if a is None:
        return "--"
    md, lo, hi, n = a
    s = "%d" % round(md)
    if rng and n > 1 and round(lo) != round(hi):
        s += "\\rng{%d}{%d}" % (round(lo), round(hi))
    return s


def ratio(a, b, digits=2):
    return ("%%.%df" % digits) % (a / b) if a and b else "--"


def tt(label):
    return "\\texttt{%s}" % label


def write(out, name, lines):
    path = os.path.join(out, name)
    with open(path, "w") as fh:
        fh.write("%% generated by scripts/make_tables.py -- do not edit\n")
        fh.write("\n".join(lines) + "\n")


def model_jain(x):
    """Jain of two equal-size classes whose per-client service ratio is x."""
    return (1 + x) ** 2 / (2 * (1 + x * x))


def solve_x(target):
    lo, hi = 0.0, 1.0
    for _ in range(80):
        mid = (lo + hi) / 2
        if model_jain(mid) < target:
            lo = mid
        else:
            hi = mid
    return hi


CONT = {"sus": "sus.", "bur": "bur."}


# ----------------------------------------------------------------- tables --

def t_motivation(res, out):
    c = load(res, "matrix-*-w8-h8-*.json")
    L = ["\\begin{tabular}{llrrrrrr}", "\\toprule",
         "cell & lock & Mops/s & svc.\\ $J$ & burden $J$ & comb.\\ p99 & other p99 & starved c/b \\\\",
         "\\midrule"]
    first = True
    for cont, b in (("sus", 0), ("sus", 31), ("bur", 0), ("bur", 31)):
        if not first:
            L.append("\\midrule")
        first = False
        for lock in ("dispatch", "ces", "fc-noyield", "fc", "fcpq"):
            runs = c.get((cont, 8, b, lock))
            if not runs:
                continue
            L.append("%s b%d & %s & %s & %s & %s & %s & %s & %s/%s \\\\" % (
                CONT[cont], b, tt(lock), f(agg(runs, "thr"), 3, 1e-6), f(agg(runs, "sj")),
                f(agg(runs, "bj")), fus(agg(runs, "comb_p99")), fus(agg(runs, "noncomb_p99")),
                fint(agg(runs, "starved_c")), fint(agg(runs, "starved_b"))))
    L += ["\\bottomrule", "\\end{tabular}"]
    write(out, "tab-motivation.tex", L)


def t_fifo(res, out):
    c = load(res, "matrix-*-w8-h8-b31-sus-*.json")
    tk = load(res, "tokio-*-w8-h8-sus-*.json")
    L = ["\\begin{tabular}{lrrrrrr}", "\\toprule",
         "lock & CS$_L$ & CS$_H$ & L:H ops & model $J$ & measured $J$ & L:H for $J{=}1$ \\\\",
         "\\midrule"]
    rows = [("dispatch", c[("sus", 8, 31, "dispatch")]), ("ces", c[("sus", 8, 31, "ces")]),
            ("fc", c[("sus", 8, 31, "fc")]), ("tokio-mutex", tk[("sus", 8, None, "tokio-mutex")])]
    for lock, runs in rows:
        x = med(runs, "lh") * med(runs, "cs_l") / med(runs, "cs_h")
        L.append("%s & %s & %s & %s & %.3f & %s & %.2f \\\\" % (
            tt(lock), f(agg(runs, "cs_l"), 0, rng=False), f(agg(runs, "cs_h"), 0, rng=False), f(agg(runs, "lh"), 2),
            model_jain(x), f(agg(runs, "sj")), med(runs, "cs_h") / med(runs, "cs_l")))
    L += ["\\bottomrule", "\\end{tabular}"]
    write(out, "tab-fifo.tex", L)


def t_burden(res, out):
    c = load(res, "p3-*-h8-*.json")
    L = ["\\begin{tabular}{llrrrrrr}", "\\toprule",
         "cell & variant & burden $J$ & comb.\\ p99 & other p99 & starved c/b & Mops/s & /\\texttt{ces} \\\\",
         "\\midrule"]
    blocks = []
    for cont, b in (("sus", 0), ("sus", 31), ("bur", 0), ("bur", 31)):
        blocks.append((8, cont, b, ("ces", "ces-k64", "ces-k64-home", "ces-t64000-home",
                                    "fc", "fc-home", "fc-remote")))
    for cont, b in (("sus", 0), ("sus", 31), ("bur", 0), ("bur", 31)):
        blocks.append((16, cont, b, ("ces", "ces-k64-home")))
    first = True
    for w, cont, b, variants in blocks:
        ref = med(c.get((cont, w, b, "ces"), []), "thr")
        if not first:
            L.append("\\midrule")
        first = False
        for v in variants:
            runs = c.get((cont, w, b, v))
            if not runs:
                continue
            assert runs[0]["window"] == "09-28"
            L.append("W%d %s b%d & %s & %s & %s & %s & %s/%s & %s & %s \\\\" % (
                w, CONT[cont], b, tt(v), f(agg(runs, "bj")), fus(agg(runs, "comb_p99")),
                fus(agg(runs, "noncomb_p99")), fint(agg(runs, "starved_c")),
                fint(agg(runs, "starved_b")), f(agg(runs, "thr"), 3, 1e-6),
                ratio(med(runs, "thr"), ref)))
    L += ["\\bottomrule", "\\end{tabular}"]
    write(out, "tab-burden.tex", L)


def service_row(label, win, runs):
    x = med(runs, "lh") * med(runs, "cs_l") / med(runs, "cs_h")
    wmax = agg(runs, "w_max")
    return "%s & %s & %s & %s & %.3f & %s & %s & %s & %s / %s & %s \\\\" % (
        label, win, f(agg(runs, "thr"), 3, 1e-6), f(agg(runs, "sj")), model_jain(x),
        f(agg(runs, "lh"), 2, rng=False), f(agg(runs, "util"), rng=False), fint(agg(runs, "o"), rng=False),
        f(agg(runs, "h_p50"), 1, rng=False), f(agg(runs, "h_p99"), 1), fint(wmax) if wmax else "--")


def t_service(res, out, nums):
    old = load(res, "p3-*-w8-h8-b31-sus-*.json")
    ab = load(res, "p3ab-pre-*-w8-h8-b31-sus-*.json")
    L = ["\\begin{tabular}{llrrrrrrrr}", "\\toprule",
         "variant & win. & Mops/s & svc.\\ $J$ & model $J$ & L:H & util & non-CS/op & heavy p50 / p99 ($\\mu$s) & max wait \\\\",
         "\\midrule"]
    for v in ("dispatch", "ces", "fc", "fc-remote", "fcpq", "fcpq-h8", "fcpq-h16", "fcpq-h16-home"):
        runs = old[("sus", 8, 31, v)]
        assert runs[0]["window"] == "09-28"
        L.append(service_row(tt(v), "09-28", runs))
    L.append("\\midrule")
    for v in ("fc-remote", "fcpq-h16-home"):
        runs = ab[("sus", 8, 31, v)]
        assert runs[0]["window"] == "09-29"
        L.append(service_row(tt(v), "09-29", runs))
    for cl in (8, 16, 32, 0):
        v = "fcpq-h16-home-c%d-nmean" % cl
        runs = old[("sus", 8, 31, v)]
        assert runs[0]["window"] == "09-29"
        L.append(service_row("\\quad" + tt("-c%d" % cl), "09-29", runs))
    L += ["\\bottomrule", "\\end{tabular}"]
    write(out, "tab-service.tex", L)

    # Confirmation cells (09-29): clamp 8 vs 16 at W8 b0 and W16 b31.
    allp3 = load(res, "p3-fcpq-h16-home-c*-nmean-*.json")
    L = ["\\begin{tabular}{llrrrrr}", "\\toprule",
         "cell & clamp & Mops/s & svc.\\ $J$ & L:H & heavy p99 ($\\mu$s) & burden $J$ \\\\",
         "\\midrule"]
    first = True
    for cont, w, b in (("sus", 8, 31), ("sus", 8, 0), ("sus", 16, 31), ("bur", 8, 31), ("bur", 16, 31)):
        if not first:
            L.append("\\midrule")
        first = False
        for cl in (8, 16):
            runs = allp3[(cont, w, b, "fcpq-h16-home-c%d-nmean" % cl)]
            L.append("W%d %s b%d & %d & %s & %s & %s & %s & %s \\\\" % (
                w, CONT[cont], b, cl, f(agg(runs, "thr"), 3, 1e-6), f(agg(runs, "sj")),
                f(agg(runs, "lh"), 2, rng=False), f(agg(runs, "h_p99"), 1, rng=False),
                f(agg(runs, "bj"), rng=False)))
    L += ["\\bottomrule", "\\end{tabular}"]
    write(out, "tab-clamp-confirm.tex", L)

    # Derived numbers for the text (clamp 8 and clamp 16, W8 b31 sus, 09-29).
    c8 = old[("sus", 8, 31, "fcpq-h16-home-c8-nmean")]
    c16 = old[("sus", 8, 31, "fcpq-h16-home-c16-nmean")]
    fcr = ab[("sus", 8, 31, "fc-remote")]
    x95 = solve_x(0.95)
    # class costs at the fair operating point (clamp 16), as in FINDINGS 09-29
    csl, csh = med(c16, "cs_l"), med(c16, "cs_h")
    lh95 = x95 * csh / csl
    cbar95 = (lh95 * csl + csh) / (lh95 + 1)
    nums["XNinetyFive"] = "%.3f" % x95
    nums["LHNinetyFive"] = "%.2f" % lh95
    nums["CbarNinetyFive"] = "%.0f" % cbar95
    nums["ONeededEighty"] = "%.0f" % (cbar95 * (1 / 0.8 - 1))
    for tag, runs in (("CEight", c8), ("CSixteen", c16), ("FcRemoteAB", fcr)):
        nums["Cbar" + tag] = "%.0f" % med(runs, "cbar")
        nums["O" + tag] = "%.0f" % med(runs, "o")
        nums["Util" + tag] = "%.3f" % med(runs, "util")
    nums["OFcOld"] = "%.0f" % med(old[("sus", 8, 31, "fc")], "o")
    nums["UtilFcOld"] = "%.3f" % med(old[("sus", 8, 31, "fc")], "util")
    o_fc = med(old[("sus", 8, 31, "fc")], "o")
    nums["UtilAtOFcOld"] = "%.3f" % (cbar95 / (cbar95 + o_fc))
    nums["PromotedCEight"] = "%.3f" % med(c8, "promoted")


XRT_ROWS = [("tokio", "tokio-mutex"), ("tokio", "tokio-mutex-unconstrained"), ("tokio", "async-lock"),
            ("tokio", "std-mutex"), ("tokio", "parking-lot"), ("coro", "dispatch"),
            ("coro", "dispatch-home"), ("coro", "ces-k64-home"), ("coro", "fc-remote"),
            ("coro", "fcpq-h16-home")]
XRT_CELLS = [(8, "sus"), (8, "bur"), (16, "sus"), (16, "bur")]


def t_xrt(res, out, nums):
    tk = load(res, "tokio-*.json")
    co = load(res, "xrt-*.json")

    def runs_of(rt, lock, w, cont):
        return (tk if rt == "tokio" else co).get((cont, w, None if rt == "tokio" else 31, lock))

    for runs in list(tk.values()) + list(co.values()):
        assert runs[0]["window"] == "09-28"
    L = ["\\begin{tabular}{l" + "rr" * len(XRT_CELLS) + "}", "\\toprule",
         " & " + " & ".join("\\multicolumn{2}{c}{W%d %s}" % (w, CONT[c]) for w, c in XRT_CELLS) + " \\\\",
         " ".join("\\cmidrule(lr){%d-%d}" % (2 + 2 * i, 3 + 2 * i) for i in range(len(XRT_CELLS))),
         "variant" + " & Mops/s & $\\times$tm" * len(XRT_CELLS) + " \\\\", "\\midrule"]
    for rt, lock in XRT_ROWS:
        if (rt, lock) == ("coro", "dispatch"):
            L.append("\\midrule")
        cells = []
        for w, cont in XRT_CELLS:
            runs = runs_of(rt, lock, w, cont)
            ref = med(runs_of("tokio", "tokio-mutex", w, cont), "thr")
            cells.append("%s & %s" % (f(agg(runs, "thr"), 3, 1e-6), ratio(med(runs, "thr"), ref)))
        L.append("%s & %s \\\\" % (tt(lock), " & ".join(cells)))
    L += ["\\bottomrule", "\\end{tabular}"]
    write(out, "tab-xrt-thr.tex", L)

    L = ["\\begin{tabular}{l" + "rr" * len(XRT_CELLS) + "}", "\\toprule",
         " & " + " & ".join("\\multicolumn{2}{c}{W%d %s}" % (w, CONT[c]) for w, c in XRT_CELLS) + " \\\\",
         " ".join("\\cmidrule(lr){%d-%d}" % (2 + 2 * i, 3 + 2 * i) for i in range(len(XRT_CELLS))),
         "variant" + " & svc.\\ $J$ & st.\\ c/b" * len(XRT_CELLS) + " \\\\", "\\midrule"]
    for rt, lock in XRT_ROWS:
        if (rt, lock) == ("coro", "dispatch"):
            L.append("\\midrule")
        cells = []
        for w, cont in XRT_CELLS:
            runs = runs_of(rt, lock, w, cont)
            cells.append("%s & %s/%s" % (f(agg(runs, "sj")), fint(agg(runs, "starved_c")),
                                         fint(agg(runs, "starved_b"))))
        L.append("%s & %s \\\\" % (tt(lock), " & ".join(cells)))
    L += ["\\bottomrule", "\\end{tabular}"]
    write(out, "tab-xrt-fair.tex", L)

    lifo = load_lifo(res)
    L = ["\\begin{tabular}{lrrrrrrr}", "\\toprule",
         " & \\multicolumn{2}{c}{\\texttt{tokio-mutex}} & \\multicolumn{2}{c}{\\texttt{async-lock}} & "
         "\\multicolumn{3}{c}{coro / \\texttt{tokio-mutex} LIFO off} \\\\",
         "\\cmidrule(lr){2-3} \\cmidrule(lr){4-5} \\cmidrule(lr){6-8}",
         "cell & LIFO on & off & on & off & \\texttt{ces-k64-home} & \\texttt{fc-remote} & \\texttt{fcpq-h16-home} \\\\",
         "\\midrule"]
    for w, cont in XRT_CELLS:
        off = med(lifo[(w, cont, "tokio-mutex", "off")], "thr")
        row = ["W%d %s" % (w, CONT[cont])]
        for lock in ("tokio-mutex", "async-lock"):
            for mode in ("on", "off"):
                row.append(f(agg(lifo[(w, cont, lock, mode)], "thr"), 3, 1e-6))
        for lock in ("ces-k64-home", "fc-remote", "fcpq-h16-home"):
            row.append(ratio(med(runs_of("coro", lock, w, cont), "thr"), off))
        L.append(" & ".join(row) + " \\\\")
    L += ["\\bottomrule", "\\end{tabular}"]
    write(out, "tab-lifo.tex", L)
    for runs in lifo.values():
        assert runs[0]["window"] == "09-28"

    def r_(lock, w, cont, base):
        return med(runs_of("coro", lock, w, cont), "thr") / base

    tm = {k: med(runs_of("tokio", "tokio-mutex", *k), "thr") for k in XRT_CELLS}
    off = {k: med(lifo[(k[0], k[1], "tokio-mutex", "off")], "thr") for k in XRT_CELLS}
    sus_on = [r_(l, w, c, tm[(w, c)]) for l in ("ces-k64-home", "fc-remote") for w, c in XRT_CELLS if c == "sus"]
    bur_on = [r_(l, w, c, tm[(w, c)]) for l in ("ces-k64-home", "fc-remote") for w, c in XRT_CELLS if c == "bur"]
    sus_off = [r_(l, w, c, off[(w, c)]) for l in ("ces-k64-home", "fc-remote") for w, c in XRT_CELLS if c == "sus"]
    bur_off = [r_(l, w, c, off[(w, c)]) for l in ("ces-k64-home", "fc-remote")
               for w, c in XRT_CELLS if c == "bur"]
    nums["SusOnLo"], nums["SusOnHi"] = "%.2f" % min(sus_on), "%.2f" % max(sus_on)
    nums["BurOnLo"], nums["BurOnHi"] = "%.2f" % min(bur_on), "%.2f" % max(bur_on)
    nums["SusOffLo"], nums["SusOffHi"] = "%.2f" % min(sus_off), "%.2f" % max(sus_off)
    nums["BurOffLo"], nums["BurOffHi"] = "%.2f" % min(bur_off), "%.2f" % max(bur_off)


def t_actor(res, out):
    ref = load(res, "actorref-*.json")
    p3 = load(res, "p3-*-h8-*.json")
    xrt = load(res, "xrt-*.json")
    L = ["\\begin{tabular}{lllrrrrr}", "\\toprule",
         "cell & variant & win. & Mops/s & /\\texttt{fc-remote} & burden $J$ & comb.\\ / other p99 ($\\mu$s) & svc.\\ $J$ \\\\",
         "\\midrule"]

    base_win = [None]

    def row(cell, v, runs, base):
        return "%s & %s & %s & %s & %s & %s & %s / %s & %s \\\\" % (
            cell, tt(v), runs[0]["window"], f(agg(runs, "thr"), 3, 1e-6),
            ratio(med(runs, "thr"), base) + ("$^\\dagger$" if runs[0]["window"] != base_win[0] else ""),
            f(agg(runs, "bj")), fus(agg(runs, "comb_p99"), rng=False), fus(agg(runs, "noncomb_p99"), rng=False),
            f(agg(runs, "sj")))

    first = True
    for cont in ("sus", "bur"):
        if not first:
            L.append("\\midrule")
        first = False
        base = med(ref[(cont, 8, 31, "fc-remote")], "thr")
        base_win[0] = ref[(cont, 8, 31, "fc-remote")][0]["window"]
        cell = "W8 %s b31" % CONT[cont]
        for v in ("dispatch", "ces-k64-home", "fc-remote"):
            L.append(row(cell, v, ref[(cont, 8, 31, v)], base))
        for v in ("actor", "actor-inline"):
            L.append(row(cell, v, p3[(cont, 8, 31, v)], base))
    for cont, w, b in (("sus", 8, 0), ("bur", 8, 0), ("sus", 16, 31), ("bur", 16, 31)):
        L.append("\\midrule")
        cell = "W%d %s b%d" % (w, CONT[cont], b)
        refruns = p3.get((cont, w, b, "fc-remote")) or xrt[(cont, w, b, "fc-remote")]
        base = med(refruns, "thr")
        base_win[0] = refruns[0]["window"]
        L.append(row(cell, "fc-remote", refruns, base))
        for v in ("actor", "actor-inline"):
            L.append(row(cell, v, p3[(cont, w, b, v)], base))
    L += ["\\bottomrule", "\\end{tabular}"]
    write(out, "tab-actor.tex", L)


LOC_GROUPS = [
    ("executor", ["coro_delegation/src/executor.rs"]),
    ("lock trait + \\texttt{dispatch}", ["coro_delegation/src/lock.rs", "coro_delegation/src/locks/dispatch.rs"]),
    ("\\texttt{ces}", ["coro_delegation/src/locks/ces.rs"]),
    ("\\texttt{fc} (delegation core)", ["coro_delegation/src/locks/fc.rs"]),
    ("\\texttt{fcpq} policy", ["coro_delegation/src/locks/fc_pq.rs"]),
    ("\\texttt{actor}, \\texttt{actor-inline}", ["coro_delegation/src/locks/actor.rs"]),
    ("stats, workload, \\texttt{coro-bench}", ["coro_delegation/src/stats.rs", "coro_delegation/src/workload.rs",
                                              "coro_delegation/src/bin/coro_bench.rs",
                                              "coro_delegation/src/lib.rs", "coro_delegation/src/locks/mod.rs"]),
    ("tokio baseline (\\texttt{tokio-bench})", ["coro_tokio_baseline/src/main.rs", "coro_tokio_baseline/src/workload.rs",
                                               "coro_tokio_baseline/src/stats.rs"]),
]


def count_loc(path):
    """(code, test) lines: non-blank lines that are not `//` comments; lines
    from the first `#[cfg(test)]` on count as test code."""
    code = test = 0
    in_test = False
    with open(path) as fh:
        for line in fh:
            s = line.strip()
            if s.startswith("#[cfg(test)]"):
                in_test = True
            if not s or s.startswith("//"):
                continue
            if in_test:
                test += 1
            else:
                code += 1
    return code, test


def t_loc(crates, out, nums):
    L = ["\\begin{tabular}{lrr}", "\\toprule", "component & code & tests \\\\", "\\midrule"]
    tot_c = tot_t = 0
    seen = set()
    for label, files in LOC_GROUPS:
        c = t = 0
        for rel in files:
            a, b = count_loc(os.path.join(crates, rel))
            c, t = c + a, t + b
            seen.add(rel)
        tot_c, tot_t = tot_c + c, tot_t + t
        L.append("%s & %d & %d \\\\" % (label, c, t))
    missing = [p for p in glob.glob(os.path.join(crates, "coro_*", "src", "**", "*.rs"), recursive=True)
               if os.path.relpath(p, crates) not in seen]
    if missing:
        sys.exit("make_tables: LOC groups miss %s" % missing)
    L += ["\\midrule", "total & %d & %d \\\\" % (tot_c, tot_t), "\\bottomrule", "\\end{tabular}"]
    write(out, "tab-loc.tex", L)
    nums["LocTotal"] = "%d" % tot_c
    nums["LocTests"] = "%d" % tot_t




def main():
    here_paper = os.path.join(HERE, "..")
    res = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here_paper, "..", "results")
    out = sys.argv[2] if len(sys.argv) > 2 else os.path.join(here_paper, "tables")
    os.makedirs(out, exist_ok=True)
    nums = {}
    t_motivation(res, out)
    t_fifo(res, out)
    t_burden(res, out)
    t_loc(os.path.join(here_paper, "..", ".."), out, nums)
    t_service(res, out, nums)
    t_xrt(res, out, nums)
    t_actor(res, out)
    write(out, "numbers.tex", ["\\newcommand{\\%s}{%s}" % (k, v) for k, v in sorted(nums.items())])
    print("make_tables: wrote %s" % ", ".join(sorted(os.listdir(out))))


if __name__ == "__main__":
    main()
