#!/usr/bin/env python3
"""Generate the paper's Typst table snippets from the result JSONs (stdlib only).

usage: make_tables.py [RESULTS_DIR] [OUT_DIR]     (defaults: ../results  tables)

Every table cell is median [min, max] over the repeats of one configuration
(bracket omitted when min == max), computed with the metric definitions of
`crates/coro_delegation/scripts/summarize.py` (throughput, Jain indices,
bystander p99s, combiner worker) and `scripts/summarize_fcpq_sweep.py` (util,
admin/op, gap/op, queue waits), which are imported, not re-derived.

Measurement windows: runs whose JSON timestamp is before 2026-09-29 00:07:54
UTC (unix 1790640474) ran at turbo clock ("09-28"); runs up to
2026-09-29 22:00 UTC (unix 1790719200) ran with CPUs 0-15 capped at 3.0 GHz
("09-29"); later runs are the "09-30" session (FINDINGS entries dated 09-30:
async CFL, co1, co2; same 3.0 GHz cap). Each table states which window its rows
come from; the script refuses to aggregate a cell that mixes windows.

The current co-pq results are `co2-*` (ordinary-waker co-pq, one frozen binary,
FINDINGS "ordinary-waker co-pq, same-window matrix"); `co1-*` used the old
inline co-pq and is not read here.

Also writes `numbers.typ`: `#let` bindings for the derived numbers the
text quotes (model Jain, required light:heavy ratio, co2 medians and ratios).
"""
import glob
import json
import os
import statistics  # noqa: F401
import sys
from collections import defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.join(HERE, "..", "..", "scripts"))
import summarize  # noqa: E402
import summarize_fcpq_sweep as sweep  # noqa: E402
import summarize_co as sc  # noqa: E402

CAP_UNIX = 1790640474  # 2026-09-29 00:07:54 UTC: 3.0 GHz cap set on CPUs 0-15
S30_UNIX = 1790719200  # 2026-09-29 22:00 UTC: start of the session labelled 09-30
CENSORED_US = 1e6       # a bystander p99 above 1 s is a censored sample


# ---------------------------------------------------------------- loading --

def window_of(r):
    t = int(r["timestamp"].split(":")[1])
    return "09-28" if t < CAP_UNIX else ("09-29" if t < S30_UNIX else "09-30")


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


# ------------------------------------------------------------- formatting --
#
# Cell text is Typst markup. `rng` (median [min, max] suffix) is defined in
# ../lib.typ, which every table snippet imports.

def agg(runs, name):
    return summarize.agg([m[name] for m in runs]) if runs else None


def med(runs, name):
    a = agg(runs, name)
    return a[0] if a else None


def rng_(lo, hi):
    return "#rng[%s][%s]" % (lo, hi)


def f(a, digits=3, scale=1.0, rng=True):
    """median#rng[min][max]; '--' if missing."""
    if a is None:
        return "--"
    md, lo, hi, n = a
    fmt = "%%.%df" % digits
    s = fmt % (md * scale)
    if rng and n > 1 and (fmt % (lo * scale)) != (fmt % (hi * scale)):
        s += rng_(fmt % (lo * scale), fmt % (hi * scale))
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
        s += rng_("%d" % round(lo), "%d" % round(hi))
    return s


def ratio(a, b, digits=2):
    return ("%%.%df" % digits) % (a / b) if a and b else "--"


def tt(label):
    return "`%s`" % label


MID = "midrule"


def span(text, n):
    """A centred header cell spanning n columns."""
    return (text, n)


def rules(*ranges):
    """Partial rules under spanning header cells; (first, last) 1-based columns as in \\cmidrule."""
    return ("rules", ranges)


def cell_src(c):
    if isinstance(c, tuple):
        return "table.cell(colspan: %d, align: center)[%s]," % (c[1], c[0])
    return "[%s]," % c


def table(spec, head, body):
    """Booktabs-style #table: `spec` like 'llrr'; `head` rows (cells or rules());
    `body` rows (lists of cells) or MID for a mid rule."""
    al = {"l": "left", "r": "right", "c": "center"}
    L = ["#table(", "  columns: %d," % len(spec),
         "  align: (%s)," % ", ".join(al[c] for c in spec),
         "  stroke: none,", "  table.hline(stroke: 0.8pt),", "  table.header("]
    for row in head:
        if isinstance(row, tuple) and row[0] == "rules":
            for a, b in row[1]:
                L.append("    table.hline(start: %d, end: %d, stroke: 0.4pt)," % (a - 1, b))
        else:
            L.append("    " + " ".join(cell_src(c) for c in row))
    L += ["  ),", "  table.hline(stroke: 0.5pt),"]
    for row in body:
        if row == MID:
            L.append("  table.hline(stroke: 0.5pt),")
        else:
            L.append("  " + " ".join(cell_src(c) for c in row))
    L += ["  table.hline(stroke: 0.8pt),", ")"]
    return L


def write(out, name, lines, imports=True):
    path = os.path.join(out, name)
    with open(path, "w") as fh:
        fh.write("// generated by scripts/make_tables.py -- do not edit\n")
        if imports:
            fh.write('#import "../lib.typ": rng\n')
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
XRT_CELLS = [(8, "sus"), (8, "bur"), (16, "sus"), (16, "bur")]  # tokio-runtime cells of the 09-28 motivation figure


# ----------------------------------------------------------------- tables --

def t_fifo(res, out):
    c = load(res, "matrix-*-w8-h8-b31-sus-*.json")
    tk = load(res, "tokio-*-w8-h8-sus-*.json")
    head = [["lock", '$"CS"_L$', '$"CS"_H$', "L:H ops", "model $J$", "measured $J$", "L:H for $J=1$"]]
    body = []
    rows = [("dispatch", c[("sus", 8, 31, "dispatch")]), ("ces", c[("sus", 8, 31, "ces")]),
            ("fc", c[("sus", 8, 31, "fc")]), ("tokio-mutex", tk[("sus", 8, None, "tokio-mutex")])]
    for lock, runs in rows:
        x = med(runs, "lh") * med(runs, "cs_l") / med(runs, "cs_h")
        body.append([tt(lock), f(agg(runs, "cs_l"), 0, rng=False), f(agg(runs, "cs_h"), 0, rng=False),
                     f(agg(runs, "lh"), 2), "%.3f" % model_jain(x), f(agg(runs, "sj")),
                     "%.2f" % (med(runs, "cs_h") / med(runs, "cs_l"))])
    write(out, "tab-fifo.typ", table("lrrrrrr", head, body))


def t_service_nums(res, nums):
    """Derived numbers of the policy text: FC-PQ clamp 8 / 16 (W8 b31 sus, 09-29)."""
    old = load(res, "p3-*-w8-h8-b31-sus-*.json")
    c8 = old[("sus", 8, 31, "fcpq-h16-home-c8-nmean")]
    c16 = old[("sus", 8, 31, "fcpq-h16-home-c16-nmean")]
    for runs in (c8, c16):
        assert runs[0]["window"] == "09-29"
    x95 = solve_x(0.95)
    # class costs at the fair operating point (clamp 16), as in FINDINGS 09-29
    csl, csh = med(c16, "cs_l"), med(c16, "cs_h")
    lh95 = x95 * csh / csl
    nums["XNinetyFive"] = "%.3f" % x95
    nums["LHNinetyFive"] = "%.2f" % lh95
    nums["CbarCEight"] = "%.0f" % med(c8, "cbar")
    nums["CbarCSixteen"] = "%.0f" % med(c16, "cbar")
    nums["PromotedCEight"] = "%.3f" % med(c8, "promoted")


def t_async_nums(res, nums):
    """async-lock monopoly: tokio runs (09-28) in which exactly one client completed an operation."""
    tk = load(res, "tokio-*.json")
    nclients = {"sus": 64, "bur": 16}
    al = [(cont, m) for (cont, w, b, lock), runs in tk.items() if lock == "async-lock" for m in runs]
    assert all(m["window"] == "09-28" for _, m in al)
    nums["AsyncMonoRuns"] = "%d" % sum(1 for c, m in al if m["starved_c"] == nclients[c] - 1)
    nums["AsyncRuns"] = "%d" % len(al)


def t_sync_nums(res, nums):
    """co-fifo with the async step-aside (`-sremote`) against a synchronous release (`-snone`, as dropping the
    guard), W8 b31, spin and yield. co-fifo is unchanged since the co1-* session (only co-pq changed), so this
    pair is read from co1-* (09-30 window, an earlier session than co2-*; ratios only within co1)."""
    for mode, glob_ in (("Spin", "co1-co-fifo-s*-k64-home-w8-h8-b31-*-r*.json"),
                        ("Yield", "co1-co-fifo-s*-k64-home-pyield-w8-h8-b31-*-r*.json")):
        c = load(res, glob_)
        for cont in ("sus", "bur"):
            a = c[(cont, 8, 31, "co-fifo-sremote-k64-home")]
            s = c[(cont, 8, 31, "co-fifo-snone-k64-home")]
            assert a[0]["window"] == s[0]["window"] == "09-30"
            tag = cont.capitalize() + mode
            nums["SyncOverAsyncThr" + tag] = "%.2f" % (med(s, "thr") / med(a, "thr"))
            nums["AsyncThr" + tag] = "%.3f" % (med(a, "thr") * 1e-6)
            nums["SyncThr" + tag] = "%.3f" % (med(s, "thr") * 1e-6)
            nums["AsyncO" + tag] = "%.0f" % med(a, "o")
            nums["SyncO" + tag] = "%.0f" % med(s, "o")


# ------------------------------------------------------------------ clamp --

def t_clamp_units(res, out):
    """The clamp in its own unit: FC-PQ passes (09-29), dispatch-pq and co-pq hand-offs (09-29 / 09-30)."""
    p3 = load(res, "p3-fcpq-h16-home-c*-nmean-*.json")
    dpq = load(res, "dpq-*.json")
    co2 = load_co2(res)
    head = [["lock", "clamp", "win.", "svc. $J$", "L:H", "max. wait"]]
    body = []

    def row(label, clamp, runs):
        wmax = agg(runs, "w_max")
        return [label, clamp, runs[0]["window"], f(agg(runs, "sj"), 3, rng=False),
                f(agg(runs, "lh"), 2, rng=False), fint(wmax, rng=False) if wmax else "--"]

    for cl, name in ((8, "8 passes"), (16, "16 passes"), (0, "off")):
        body.append(row(tt("fcpq-h16-home"), name, p3[("sus", 8, 31, "fcpq-h16-home-c%d-nmean" % cl)]))
    body.append(MID)
    for v, name, lab in (("dispatch-pq-home", "16 hand-offs", "`dispatch-pq-home`"),
                         ("dispatch-pq-home-c256", "256 hand-offs", "`dispatch-pq-home`"),
                         ("dispatch-pq-c0", "off", "`dispatch-pq`")):
        body.append(row(lab, name, dpq[("sus", 8, 31, v)]))
    body.append(MID)
    for v, name in (("co-pq-sremote-k64-c256", "256 hand-offs"), ("co-pq-sremote-k64-c0", "off")):
        body.append(row(tt("co-pq"), name, co2[("sus", 8, "spin", v)]))
    write(out, "tab-clamp.typ", table("llrrrr", head, body))


# ------------------------------------------------------------------- co2 --
#
# The ordinary-waker co-pq matrix (results/co2-*, FINDINGS 2026-09-30
# "ordinary-waker co-pq, same-window matrix"): coro-bench and tokio-bench
# from one session. Keys: (contention, W, parallel mode, JSON lock label).

CO2_LOCKS = [  # (JSON label, printed name, Typst-variable stem)
    ("co-pq-sremote-k64-c256", "co-pq", "CoPq"),
    ("co-pq-sremote-k64-c0", "co-pq-c0", "CoPqC0"),
    ("dispatch-pq-home-c256", "dispatch-pq-home-c256", "Dpq"),
    ("fcpq-h16-home-c16", "fcpq-h16-home-c16", "Fcpq"),
    ("dispatch", "dispatch", "Disp"),
    ("tokio-mutex", "tokio-mutex", "Tok"),
    ("co-fifo-sremote-k64-home", "co-fifo", "CoFifo"),
    ("ces-k64-home", "ces-k64-home", "Ces"),
    ("fc-remote", "fc-remote", "Fc"),
]
CO2_NAME = {lab: (name, stem) for lab, name, stem in CO2_LOCKS}


def co2_metrics(r, tokio):
    """The metrics of summarize_co.py (coro) or the same set for a tokio run."""
    if not tokio:
        m = sc.run_metrics(r)
    else:
        hz = r["tsc_hz"]
        t = r["measured_secs"] * hz
        lc, hc = r["classes"]["light"], r["classes"]["heavy"]
        cs = lc["service_cycles"] + hc["service_cycles"]
        m = {"thr": r["throughput_ops_per_s"], "jain": r["service_jain"], "o": (t - cs) / r["total_ops"],
             "lh": lc["ops"] / hc["ops"], "l_p99": summarize.us(lc["run_latency"]["p99"], hz),
             "h_p99": summarize.us(hc["run_latency"]["p99"], hz), "burden": None,
             "byst_p99": summarize.us(r["bystander_latency"]["p99"], hz), "starved_c": r["starved_clients"],
             "starved_b": r.get("starved_bystanders"), "w_max": None}
    m["window"] = window_of(r)
    return m


def load_co2(results):
    """{(cont, W, mode, label): [metrics]} for every co2-*.json; all must be 09-30."""
    cells = defaultdict(list)
    files = sorted(glob.glob(os.path.join(results, "co2-*.json")))
    if not files:
        sys.exit("make_tables: no co2-*.json in %s" % results)
    for fn in files:
        with open(fn) as fh:
            r = json.load(fh)
        cfg = r["config"]
        m = co2_metrics(r, os.path.basename(fn).startswith("co2-tokio-"))
        if m["window"] != "09-30":
            sys.exit("make_tables: %s is not in the 09-30 window" % fn)
        m["sj"] = m["jain"]
        cells[(summarize.contention_label(cfg), cfg["workers"], cfg["parallel_mode"], r["lock"])].append(m)
    return cells


def t_co2(res, out, nums):
    c = load_co2(res)
    fair = ["co-pq-sremote-k64-c256", "dispatch-pq-home-c256", "fcpq-h16-home-c16"]
    fifo = ["dispatch", "tokio-mutex", "co-fifo-sremote-k64-home", "ces-k64-home", "fc-remote"]
    groups = [("sus", "spin", 4), ("sus", "yield", 3), ("bur", "spin", 3), ("bur", "yield", 3)]
    ttl = {"sus": "sustained", "bur": "bursty"}
    head = [[""] + [span("%s, %s" % (ttl[k], m), n) for k, m, n in groups],
            rules((2, 5), (6, 8), (9, 11), (12, 14)),
            ["lock"] + ["Mops/s", "$o$", "$J$", "L:H"] + ["Mops/s", "$o$", "$J$"] * 3]
    body = []
    for block in (fair, fifo):
        if body:
            body.append(MID)
        for lab in block:
            row = [tt(CO2_NAME[lab][0])]
            for cont, mode, n in groups:
                runs = c[(cont, 8, mode, lab)]
                row += [f(agg(runs, "thr"), 3, 1e-6, rng=False), fint(agg(runs, "o"), rng=False),
                        f(agg(runs, "jain"), 3, rng=False)]
                if n == 4:
                    row.append(f(agg(runs, "lh"), 2, rng=False))
            body.append(row)
    write(out, "tab-co2.typ", table("l" + "r" * 13, head, body))

    # Every cell with every metric (appendix).
    head = [["cell", "par", "lock", "Mops/s", "svc. $J$", "L:H", "$o$", "light / heavy p99 (µs)", "burden $J$",
             "byst. p99 (µs)", "starved c/b"]]
    body = []
    for cont, w in (("sus", 8), ("sus", 16), ("bur", 8)):
        for mode in ("spin", "yield"):
            if body:
                body.append(MID)
            for lab, name, _ in CO2_LOCKS:
                runs = c.get((cont, w, mode, lab))
                if not runs:
                    continue
                body.append(["W%d %s" % (w, CONT[cont]), mode, tt(name), f(agg(runs, "thr"), 3, 1e-6),
                             f(agg(runs, "jain"), 3, rng=False), f(agg(runs, "lh"), 2, rng=False),
                             fint(agg(runs, "o")),
                             "%s / %s" % (fint(agg(runs, "l_p99"), rng=False), fint(agg(runs, "h_p99"), rng=False)),
                             f(agg(runs, "burden"), 3, rng=False), f(agg(runs, "byst_p99"), 1, rng=False),
                             "%s/%s" % (fint(agg(runs, "starved_c"), rng=False),
                                        fint(agg(runs, "starved_b"), rng=False))])
    write(out, "tab-co2-detail.typ", table("lllrrrrrrrr", head, body))

    # Numbers the text quotes: medians per cell, and the ratios between them.
    def m_(cont, w, mode, lab, name):
        return med(c[(cont, w, mode, lab)], name)

    for (cont, w, mode, lab), runs in c.items():
        stem = CO2_NAME[lab][1]
        tag = "%s%s%s%s" % ("W16" if w == 16 else "", cont.capitalize(), mode.capitalize(), stem)
        nums["Thr" + tag] = "%.3f" % (med(runs, "thr") * 1e-6)
        nums["O" + tag] = "%.0f" % med(runs, "o")
        nums["J" + tag] = "%.3f" % med(runs, "jain")
        nums["LH" + tag] = "%.2f" % med(runs, "lh")
        wm = agg(runs, "w_max")
        if wm:
            nums["Wait" + tag] = "%.0f" % wm[0]

    def rat(cont, mode, a, b, name, w=8):
        return m_(cont, w, mode, a, name) / m_(cont, w, mode, b, name)

    PQ, FPQ, DPQ = "co-pq-sremote-k64-c256", "fcpq-h16-home-c16", "dispatch-pq-home-c256"
    FIFO_IN, DISP, TOK = "co-fifo-sremote-k64-home", "dispatch", "tokio-mutex"
    for mode in ("spin", "yield"):
        M = mode.capitalize()
        nums["PqOverFcpqThrSus" + M] = "%.2f" % rat("sus", mode, PQ, FPQ, "thr")
        nums["PqOverDpqThrSus" + M] = "%.2f" % rat("sus", mode, PQ, DPQ, "thr")
        nums["PqOverFcpqOSus" + M] = "%.1f" % rat("sus", mode, PQ, FPQ, "o")
        nums["PqOverDpqOSus" + M] = "%.2f" % rat("sus", mode, PQ, DPQ, "o")
        nums["DispOverCoFifoOSus" + M] = "%.1f" % rat("sus", mode, DISP, FIFO_IN, "o")
        nums["CoFifoOverDispThrSus" + M] = "%.2f" % rat("sus", mode, FIFO_IN, DISP, "thr")
        nums["CoFifoOverCesThrSus" + M] = "%.2f" % rat("sus", mode, FIFO_IN, "ces-k64-home", "thr")
        nums["FcOverDispThrSus" + M] = "%.2f" % rat("sus", mode, "fc-remote", DISP, "thr")
        nums["PqOverCesThrBur" + M] = "%.2f" % rat("bur", mode, PQ, "ces-k64-home", "thr")
        nums["FcpqOverPqThrBur" + M] = "%.2f" % rat("bur", mode, FPQ, PQ, "thr")
        nums["TokOverDispThrSus" + M] = "%.2f" % rat("sus", mode, TOK, DISP, "thr")
        nums["TokOverDispThrBur" + M] = "%.2f" % rat("bur", mode, TOK, DISP, "thr")
        nums["DispOverCesThrBur" + M] = "%.2f" % rat("bur", mode, DISP, "ces-k64-home", "thr")
    nums["CoFifoOverFcThrSusSpin"] = "%.2f" % rat("sus", "spin", FIFO_IN, "fc-remote", "thr")
    nums["DpqOverFcpqOSusSpin"] = "%.1f" % rat("sus", "spin", DPQ, FPQ, "o")
    for lab, _, stem in CO2_LOCKS:
        for cont in ("sus", "bur"):
            nums["YieldOverSpin%s%s" % (cont.capitalize(), stem)] = "%.2f" % (
                m_(cont, 8, "yield", lab, "thr") / m_(cont, 8, "spin", lab, "thr"))
    nums["TokYieldOverSpinSus"] = "%.2f" % (m_("sus", 8, "yield", TOK, "thr") / m_("sus", 8, "spin", TOK, "thr"))
    nums["TokYieldOverSpinBur"] = "%.2f" % (m_("bur", 8, "yield", TOK, "thr") / m_("bur", 8, "spin", TOK, "thr"))
    pq_cells = {k: runs for k, runs in c.items() if k[3].startswith("co-pq")}
    nums["CoPqJainMin"] = "%.3f" % min(med(runs, "jain") for runs in pq_cells.values())
    nums["CoPqCells"] = "%d" % len(pq_cells)
    nums["CoPqLHLo"] = "%.2f" % min(med(runs, "lh") for runs in pq_cells.values())
    nums["CoPqLHHi"] = "%.2f" % max(med(runs, "lh") for runs in pq_cells.values())
    nums["Co2Starved"] = "%d" % sum(m["starved_c"] + (m["starved_b"] or 0) for runs in c.values() for m in runs)
    nums["Co2Runs"] = "%d" % sum(len(runs) for runs in c.values())
    burd = [med(runs, "burden") for (cont, w, mode, lab), runs in c.items()
            if cont == "sus" and w == 8 and CO2_NAME[lab][1] in ("CoFifo", "Ces", "Fc", "CoPq", "CoPqC0", "Fcpq")]
    nums["BurdenSusLo"], nums["BurdenSusHi"] = "%.3f" % min(burd), "%.3f" % max(burd)


LOC_GROUPS = [
    ("executor", ["coro_delegation/src/executor.rs"]),
    ("lock trait + `dispatch`", ["coro_delegation/src/lock.rs", "coro_delegation/src/locks/dispatch.rs"]),
    ("`ces`", ["coro_delegation/src/locks/ces.rs"]),
    ("`fc` (delegation core)", ["coro_delegation/src/locks/fc.rs"]),
    ("`UsageQueue` + `fcpq` policy", ["coro_delegation/src/locks/fc_pq.rs"]),
    ("`dispatch-pq`", ["coro_delegation/src/locks/dispatch_pq.rs"]),
    ("`actor`, `actor-inline`, `cfl` (references)", ["coro_delegation/src/locks/actor.rs",
                                                       "coro_delegation/src/locks/cfl.rs"]),
    ("`co-fifo`, `co-pq`", ["coro_delegation/src/locks/co_mutex.rs"]),
    ("stats, workload, `coro-bench`", ["coro_delegation/src/stats.rs", "coro_delegation/src/workload.rs",
                                       "coro_delegation/src/bin/coro_bench.rs",
                                       "coro_delegation/src/lib.rs", "coro_delegation/src/locks/mod.rs"]),
    ("tokio baseline (`tokio-bench`)", ["coro_tokio_baseline/src/main.rs", "coro_tokio_baseline/src/workload.rs",
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
    head = [["component", "code", "tests"]]
    body = []
    tot_c = tot_t = 0
    seen = set()
    for label, files in LOC_GROUPS:
        c = t = 0
        for rel in files:
            a, b = count_loc(os.path.join(crates, rel))
            c, t = c + a, t + b
            seen.add(rel)
        tot_c, tot_t = tot_c + c, tot_t + t
        body.append([label, "%d" % c, "%d" % t])
    missing = [p for p in glob.glob(os.path.join(crates, "coro_*", "src", "**", "*.rs"), recursive=True)
               if os.path.relpath(p, crates) not in seen]
    if missing:
        sys.exit("make_tables: LOC groups miss %s" % missing)
    body += [MID, ["total", "%d" % tot_c, "%d" % tot_t]]
    write(out, "tab-loc.typ", table("lrr", head, body))
    nums["LocTotal"] = "%d" % tot_c
    nums["LocTests"] = "%d" % tot_t


def main():
    here_paper = os.path.join(HERE, "..")
    res = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here_paper, "..", "results")
    out = sys.argv[2] if len(sys.argv) > 2 else os.path.join(here_paper, "tables")
    os.makedirs(out, exist_ok=True)
    nums = {}
    t_fifo(res, out)
    t_loc(os.path.join(here_paper, "..", ".."), out, nums)
    t_service_nums(res, nums)
    t_async_nums(res, nums)
    t_sync_nums(res, nums)
    t_clamp_units(res, out)
    t_co2(res, out, nums)
    write(out, "numbers.typ", ['#let %s = "%s"' % (k, v) for k, v in sorted(nums.items())], imports=False)
    print("make_tables: wrote %s" % ", ".join(sorted(os.listdir(out))))


if __name__ == "__main__":
    main()
