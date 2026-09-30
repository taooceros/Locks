#!/usr/bin/env python3
"""Generate the paper's data figures (vector SVG) from the result JSONs.

usage: make_figures.py [RESULTS_DIR] [OUT_DIR]     (defaults: ../results  figures)

Every figure is built with the loaders and metric definitions of
make_tables.py (which imports summarize.py and summarize_fcpq_sweep.py), so a
figure and a table built from the same cell show the same numbers. A bar or
marker is the median over the repeats of one configuration; error bars span
[min, max] over the repeats. Ratios divide by the median of the reference.

Measurement windows: make_tables.load() refuses to aggregate a cell that
mixes the 09-28 (turbo), 09-29 (3.0 GHz cap) and 09-30 (same cap, later
session) windows. Where one axis shows cells from several windows, later
cells are drawn as hollow markers; panels that hold a single window name it
in their title. fig-co2.svg is built from the co2-* files only (09-30).

Output: fig-motivation.svg, fig-burden.svg, fig-service.svg, fig-co2.svg. Widths: one column 3.3 in, two
columns 7 in; the paper includes them at that width, so the 7.5-8 pt text is printed at size.
Needs matplotlib; uses Latin Modern (the body font's design) when a TeX
installation provides it and falls back to DejaVu otherwise.
"""
import glob
import json
import math
import os
import subprocess
import sys
from collections import defaultdict

import matplotlib

matplotlib.use("svg")
import matplotlib.pyplot as plt  # noqa: E402
from matplotlib import font_manager, ticker  # noqa: E402
from matplotlib.lines import Line2D  # noqa: E402
from matplotlib.patches import Patch  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
sys.dont_write_bytecode = True
sys.path.insert(0, HERE)
import make_tables as mt  # noqa: E402

ONE, TWO = 3.3, 7.0  # inches

# Okabe-Ito, colour-blind safe. One colour per lock across all figures.
BLACK, ORANGE, SKY, GREEN = "#000000", "#E69F00", "#56B4E9", "#009E73"
YELLOW, BLUE, VERM, PURPLE, GREY = "#F0E442", "#0072B2", "#D55E00", "#CC79A7", "#8C8C8C"
COL = {
    "dispatch": GREY, "dispatch-home": "#C8C8C8",
    "ces": VERM, "ces-k64": ORANGE, "ces-k64-home": BLUE, "ces-t64000-home": SKY,
    "fc": "#505050", "fc-home": YELLOW, "fc-remote": GREEN,
    "fcpq": PURPLE, "fcpq-h16-home": PURPLE,
    "tokio-mutex": BLACK, "async-lock": "#B0B0B0", "std-mutex": "#D8D8D8", "parking-lot": "#EFEFEF",
    "actor": ORANGE, "actor-inline": SKY,
}
EDGE = dict(edgecolor="black", linewidth=0.4)
ERR = dict(ecolor="black", elinewidth=0.6, capsize=1.3, capthick=0.6)


# ------------------------------------------------------------------ style --

def lm_dir():
    try:
        p = subprocess.run(["kpsewhich", "lmroman10-regular.otf"], capture_output=True, text=True).stdout.strip()
        if p:
            return os.path.dirname(p)
    except OSError:
        pass
    for d in glob.glob("/usr/share/texmf*/fonts/opentype/public/lm") + \
            glob.glob("/usr/share/texlive/texmf-dist/fonts/opentype/public/lm"):
        return d
    return None


def setup_style():
    d = lm_dir()
    if d:
        for f in ("lmroman10-regular", "lmroman10-italic", "lmroman10-bold", "lmmono10-regular"):
            p = os.path.join(d, f + ".otf")
            if os.path.exists(p):
                font_manager.fontManager.addfont(p)
    plt.rcParams.update({
        "font.family": "serif",
        "font.serif": ["Latin Modern Roman", "DejaVu Serif"],
        "font.monospace": ["Latin Modern Mono", "DejaVu Sans Mono"],
        "mathtext.fontset": "cm",
        "font.size": 8, "axes.titlesize": 8, "axes.labelsize": 8,
        "xtick.labelsize": 7.5, "ytick.labelsize": 7.5, "legend.fontsize": 7.5,
        "axes.linewidth": 0.6, "axes.titlepad": 3,
        "xtick.major.width": 0.6, "ytick.major.width": 0.6, "xtick.minor.width": 0.4, "ytick.minor.width": 0.4,
        "xtick.major.size": 2.5, "ytick.major.size": 2.5, "xtick.minor.size": 1.5, "ytick.minor.size": 1.5,
        "xtick.major.pad": 2, "ytick.major.pad": 2, "axes.labelpad": 2,
        "lines.linewidth": 1.0, "lines.markersize": 4, "lines.markeredgewidth": 0.7,
        "axes.spines.top": False, "axes.spines.right": False,
        "axes.axisbelow": True, "axes.grid": True, "axes.grid.axis": "y",
        "grid.linewidth": 0.4, "grid.color": "#E0E0E0",
        "legend.frameon": False, "legend.handlelength": 1.3, "legend.handletextpad": 0.4,
        "legend.borderaxespad": 0.2, "legend.columnspacing": 0.9, "legend.labelspacing": 0.25,
        "hatch.linewidth": 0.5, "hatch.color": "black",
        "svg.fonttype": "path", "svg.hashsalt": "coro-delegation-paper",
        "figure.constrained_layout.h_pad": 0.02, "figure.constrained_layout.w_pad": 0.02,
        "figure.constrained_layout.hspace": 0.04, "figure.constrained_layout.wspace": 0.03,
    })


def mono(size=7.5):
    return font_manager.FontProperties(family="monospace", size=size)


def save(fig, out, name):
    fig.savefig(os.path.join(out, name), format="svg", metadata={"Date": None, "Creator": None})
    plt.close(fig)


def panel(ax, letter, title):
    ax.set_title("(%s) %s" % (letter, title), loc="left")


# ------------------------------------------------------------------- data --

def agg(runs, name):
    a = mt.agg(runs, name)
    return a[:3] if a else None


def med(runs, name):
    return mt.med(runs, name)


def yerr(a, scale=1.0):
    m, lo, hi = a
    return [[(m - lo) * scale], [(hi - m) * scale]]


def load_raw(res, pattern):
    """{(cont, W, B, lock): [json]}, windows checked like make_tables.load()."""
    cells = defaultdict(list)
    files = sorted(glob.glob(os.path.join(res, pattern)))
    if not files:
        sys.exit("make_figures: no files match %s" % os.path.join(res, pattern))
    for f in files:
        with open(f) as fh:
            r = json.load(fh)
        cfg = r["config"]
        cells[(mt.summarize.contention_label(cfg), cfg["workers"], cfg.get("balance_interval"), r["lock"])].append(r)
    for k, runs in cells.items():
        if len({mt.window_of(r) for r in runs}) != 1:
            sys.exit("make_figures: cell %s in %s mixes windows" % (k, pattern))
    return cells


def per_index(values):
    """[[v_i per run]] -> per-index (median, min, max) over runs."""
    out = []
    for col in zip(*values):
        s = sorted(col)
        out.append((s[len(s) // 2] if len(s) % 2 else (s[len(s) // 2 - 1] + s[len(s) // 2]) / 2, s[0], s[-1]))
    return out


# ---------------------------------------------------------------- figures --

def fig_motivation(res, out):
    """Subversion forms: (a) FIFO service share and its repair by usage order, (b) monopoly and
    worker seizure, (c, d) combiner burden and its bystander."""
    raw = load_raw(res, "p3-*-w8-h8-b0-sus-*.json")
    met = mt.load(res, "p3-*-w8-h8-b0-sus-*.json")
    tk = mt.load(res, "tokio-*.json")
    fig = plt.figure(figsize=(TWO, 2.95), layout="constrained")
    gt = fig.add_gridspec(2, 1, height_ratios=[1.0, 0.95])
    g_top = gt[0].subgridspec(1, 2, width_ratios=[1.75, 1.25])
    g_bot = gt[1].subgridspec(1, 2, width_ratios=[1.35, 0.9])
    cells = mt.XRT_CELLS
    cell_lab = ["W%d\n%s" % (w, {"sus": "sus.", "bur": "bur."}[c]) for w, c in cells]

    # (a) per-client service under FIFO vs. usage ordering (W8 sus b31)
    ax = fig.add_subplot(g_top[0])
    series = [("dispatch", load_raw(res, "p3-dispatch-w8-h8-b31-sus-*.json")[("sus", 8, 31, "dispatch")],
               dict(marker="o", color=GREY, mfc=GREY, ms=2.4), -0.22),
              ("tokio-mutex", load_raw(res, "tokio-tokio-mutex-w8-h8-sus-*.json")[("sus", 8, None, "tokio-mutex")],
               dict(marker="x", color=BLACK, mfc=BLACK, ms=2.6), 0.22),
              ("co-pq", load_raw(res, "co2-co-pq-c256-w8-h8-b31-sus-spin-r*.json")[
                  ("sus", 8, 31, "co-pq-sremote-k64-c256")], dict(marker="^", color=PURPLE, mfc="white", ms=2.6),
               0.0)]
    handles = []
    for name, runs, style, dx in series:
        order = sorted(runs[0]["clients"], key=lambda c: (c["class"] != "light", c["id"]))
        ids = [c["id"] for c in order]
        norm = []
        for r in runs:
            by = {c["id"]: c["service_cycles"] for c in r["clients"]}
            mean = sum(by.values()) / len(by)
            norm.append([by[i] / mean for i in ids])
        pi = per_index(norm)
        xs = [k + dx for k in range(len(ids))]
        ax.errorbar(xs, [p[0] for p in pi], yerr=[[p[0] - p[1] for p in pi], [p[2] - p[0] for p in pi]],
                    fmt=style["marker"], color=style["color"], mfc=style["mfc"], mec=style["color"], ms=style["ms"],
                    mew=0.6, elinewidth=0.5, capsize=0, ecolor=style["color"])
        sj = med([mt.metrics(r) for r in runs], "sj")
        handles.append(Line2D([], [], ls="", marker=style["marker"], color=style["color"], mfc=style["mfc"],
                              ms=3.5, label="%-17s %.3f" % (name, sj)))
    d = [mt.metrics(r) for r in series[0][1]]
    csl, csh = med(d, "cs_l"), med(d, "cs_h")
    lo, hi = 2 * csl / (csl + csh), 2 * csh / (csl + csh)
    ax.plot([-0.5, 31.5], [lo, lo], ls="--", color="black", lw=0.7, zorder=5)
    ax.plot([31.5, 63.5], [hi, hi], ls="--", color="black", lw=0.7, zorder=5)
    ax.axhline(1, ls=":", color="black", lw=0.7)
    ax.axvline(31.5, color="#BBBBBB", lw=0.5)
    ax.text(15.5, lo + 0.06, "FIFO theory: $2C_L/(C_L+C_H)$", ha="center", va="bottom", fontsize=7)
    ax.text(63.3, hi - 0.07, "FIFO theory: $2C_H/(C_L+C_H)$", ha="right", va="top", fontsize=7)
    ax.text(47.5, 0.97, "equal lock time", ha="center", va="top", fontsize=7)
    ax.text(63.3, 0.1, "hollow: 09-30 window (3.0 GHz cap)", ha="right", va="bottom", fontsize=7)
    ax.set_xlim(-1, 64)
    ax.set_ylim(0, 3.0)
    ax.set_yticks([0, 0.5, 1, 1.5, 2, 2.5])
    ax.set_xticks([15.5, 47.5])
    ax.set_xticklabels(["32 light clients (CS 1×)", "32 heavy clients (CS 8×)"])
    ax.tick_params(axis="x", length=0)
    ax.set_ylabel("client CS time / mean")
    ax.legend(handles=handles, prop=mono(7), loc="upper left", ncol=1, title="service Jain", title_fontsize=7)
    panel(ax, "a", "FIFO service unfairness (W8, sus., b31)")

    # (b) tokio-runtime locks: share of clients (bars) and bystanders (diamonds) starved
    ax = fig.add_subplot(g_top[1])
    locks = ("async-lock", "std-mutex", "parking-lot", "tokio-mutex")
    wd = 0.2
    nclients = {"sus": 64, "bur": 16}
    for g, (w, cont) in enumerate(cells):
        for i, lock in enumerate(locks):
            runs = tk[(cont, w, None, lock)]
            assert runs[0]["window"] == "09-28"
            x = g + (i - 1.5) * wd
            a = tuple(v / nclients[cont] for v in agg(runs, "starved_c"))
            ax.bar(x, max(a[0], 0.004), wd, color=COL[lock], yerr=yerr(a), error_kw=ERR, **EDGE)
            b = med(runs, "starved_b") / w
            ax.plot(x, b, marker="D", color="white", mec="black", mew=0.7, ms=3.2, zorder=6)
    ax.set_xticks(range(len(cells)))
    ax.set_xticklabels(cell_lab)
    ax.tick_params(axis="x", length=0)
    ax.set_xlim(-0.5, len(cells) - 0.5)
    ax.set_ylim(0, 1.62)
    ax.set_yticks([0, 0.25, 0.5, 0.75, 1])
    for g in range(1, len(cells)):
        ax.axvline(g - 0.5, color="#BBBBBB", lw=0.5)
    ax.set_ylabel("share starved (0 ops in 2 s)")
    h = [Patch(facecolor=COL[l], label=l, **EDGE) for l in locks]
    h.append(Line2D([], [], ls="", marker="D", color="white", mec="black", mew=0.7, ms=3.2, label="bystanders"))
    leg = ax.legend(handles=h, prop=mono(7), loc="upper center", ncol=3, bbox_to_anchor=(0.5, 1.03))
    leg.get_texts()[-1].set_fontproperties(font_manager.FontProperties(family="serif", size=7))
    panel(ax, "b", "Monopoly and worker seizure (tokio)")

    # (c) per-worker share of combining cycles, workers sorted by share
    ax = fig.add_subplot(g_bot[0])
    variants = ("ces", "ces-k64", "ces-k64-home", "fc-remote")
    wd = 0.2
    for i, v in enumerate(variants):
        runs = raw[("sus", 8, 0, v)]
        shares = []
        for r in runs:
            tot = sum(w["combining_cycles"] for w in r["workers"])
            shares.append(sorted((w["combining_cycles"] / tot for w in r["workers"]), reverse=True))
        pi = per_index(shares)
        xs = [k + 1 + (i - 1.5) * wd for k in range(8)]
        ax.bar(xs, [p[0] for p in pi], wd, color=COL[v], **EDGE,
               label="%-12s J=%.3f" % (v, med(met[("sus", 8, 0, v)], "bj")),
               yerr=[[p[0] - p[1] for p in pi], [p[2] - p[0] for p in pi]], error_kw=ERR)
    ax.axhline(1 / 8, ls="--", color="black", lw=0.6)
    ax.text(1.5, 1 / 8 + 0.02, "fair", ha="left", va="bottom", fontsize=7)
    ax.set_xticks(range(1, 9))
    ax.set_xlim(0.45, 8.55)
    ax.set_ylim(0, 1.04)
    ax.set_xlabel("worker, by combining share")
    ax.set_ylabel("share of combining cycles")
    ax.legend(prop=mono(7), loc="upper right", title="burden Jain", title_fontsize=7)
    panel(ax, "c", "Combiner burden (W8, sus., b0)")

    # (e) bystander p99 on the combiner worker vs. the other workers
    ax = fig.add_subplot(g_bot[1])
    ceil = 8e3
    for i, v in enumerate(variants):
        runs = met[("sus", 8, 0, v)]
        c, o = agg(runs, "comb_p99"), agg(runs, "noncomb_p99")
        ax.errorbar(i + 0.12, o[0], yerr=yerr(o), fmt="o", mfc="white", mec=COL[v], ms=4.5, **ERR)
        if c[0] > mt.CENSORED_US:
            ax.plot(i - 0.12, ceil, marker="^", color=COL[v], ms=5)
            ax.text(i + 0.12, ceil, "never polled", fontsize=7, va="center", ha="left")
        else:
            ax.errorbar(i - 0.12, c[0], yerr=yerr(c), fmt="o", color=COL[v], ms=4.5, **ERR)
    ax.set_yscale("log")
    ax.set_ylim(1, 3e4)
    ax.set_xlim(-0.5, len(variants) - 0.5)
    ax.set_xticks(range(len(variants)))
    ax.set_xticklabels(variants, fontproperties=mono(7), rotation=35, ha="right", rotation_mode="anchor")
    ax.set_ylabel("bystander p99 (µs)")
    ax.legend(handles=[Line2D([], [], ls="", marker="o", color="black", ms=4, label="combiner worker"),
                       Line2D([], [], ls="", marker="o", mfc="white", mec="black", ms=4, label="max. other")],
              loc="upper right", bbox_to_anchor=(1.0, 0.8), fontsize=7)
    panel(ax, "d", "Its bystander")
    save(fig, out, "fig-motivation.svg")


BURDEN_VARIANTS = ("ces", "ces-k64", "ces-k64-home", "ces-t64000-home", "fc", "fc-home", "fc-remote")
CELLS4 = (("sus", 0), ("sus", 31), ("bur", 0), ("bur", 31))
CELL_NAME = {("sus", 0): "sustained, b0", ("sus", 31): "sustained, b31",
             ("bur", 0): "bursty, b0", ("bur", 31): "bursty, b31"}


def fig_burden(res, out):
    """Burden Jain and combiner-worker bystander p99, variants x cells, W8 (+W16)."""
    c = mt.load(res, "p3-*-h8-*.json")
    fig, axs = plt.subplots(2, 4, figsize=(TWO, 2.35), layout="constrained", sharex=True, sharey="row",
                            gridspec_kw=dict(height_ratios=[1, 1.15]))
    top, bot = 8e3, 0.05
    n = len(BURDEN_VARIANTS)

    def p99s(ax, x, cp, op, col, mk, ms):
        ax.errorbar(x[1], op[0], yerr=yerr(op), fmt=mk, mfc="white", mec=col, ms=ms, **ERR)
        if cp is None:
            ax.plot(x[0], bot * 1.6, marker="x", color=col, ms=ms, mew=1.0)
        elif cp[0] > mt.CENSORED_US:
            ax.plot(x[0], top, marker="^", color=col, ms=ms + 0.6)
        else:
            ax.errorbar(x[0], cp[0], yerr=yerr(cp), fmt=mk, color=col, ms=ms, **ERR)

    for j, (cont, b) in enumerate(CELLS4):
        a1, a2 = axs[0, j], axs[1, j]
        for i, v in enumerate(BURDEN_VARIANTS):
            runs = c[(cont, 8, b, v)]
            assert runs[0]["window"] == "09-28"
            bj = agg(runs, "bj")
            a1.bar(i, bj[0], 0.75, color=COL[v], yerr=yerr(bj), error_kw=ERR, **EDGE)
            w16 = c.get((cont, 16, b, v))
            xs8 = (i - 0.3, i - 0.1) if w16 else (i - 0.1, i + 0.1)
            p99s(a2, xs8, agg(runs, "comb_p99"), agg(runs, "noncomb_p99"), COL[v], "o", 3.6)
            if w16:
                bj16 = agg(w16, "bj")
                a1.errorbar(i + 0.25, bj16[0], yerr=yerr(bj16), fmt="D", mfc="white", mec="black", ms=3.4, **ERR)
                p99s(a2, (i + 0.1, i + 0.3), agg(w16, "comb_p99"), agg(w16, "noncomb_p99"), COL[v], "D", 3.2)
        a1.axhline(0.9, ls="--", color="black", lw=0.6)
        a1.set_ylim(0, 1.08)
        a1.set_title(CELL_NAME[(cont, b)])
        a2.set_yscale("log")
        a2.set_ylim(bot, 3e4)
        a2.set_xlim(-0.6, n - 0.4)
        a2.set_xticks([])
        a2.grid(axis="y", which="minor", visible=False)
    axs[0, 0].set_ylabel("burden Jain")
    axs[1, 0].set_ylabel("bystander p99 (µs)")
    axs[0, 0].text(-0.5, 0.91, "G3", ha="left", va="bottom", fontsize=7)
    axs[1, 0].text(0.35, top, "never polled", fontsize=7, va="center", ha="left")
    lh = [Patch(facecolor=COL[v], label=v, **EDGE) for v in BURDEN_VARIANTS]
    fig.legend(handles=lh, prop=mono(7.5), loc="outside upper center", ncol=7)
    mh = [Line2D([], [], ls="", marker="o", color="black", ms=3.6, label="combiner worker"),
          Line2D([], [], ls="", marker="o", mfc="white", mec="black", ms=3.6, label="max. other worker"),
          Line2D([], [], ls="", marker="D", mfc="white", mec="black", ms=3.2, label="$W$=16 (else $W$=8)"),
          Line2D([], [], ls="", marker="^", color="black", ms=4.2, label="censored (never polled)"),
          Line2D([], [], ls="", marker="x", color="black", ms=3.6, mew=1.0, label="no bystander left (evicted)")]
    fig.legend(handles=mh, loc="outside lower center", ncol=5, fontsize=7.5)
    save(fig, out, "fig-burden.svg")


CLAMPS = (8, 16, 32, 0)


def fig_service(res, out):
    """Pass limit (09-28) and starvation-clamp sweep (09-29) of FC-PQ, W8 sus b31."""
    old = mt.load(res, "p3-*-w8-h8-b31-sus-*.json")
    ab = mt.load(res, "p3ab-pre-*-w8-h8-b31-sus-*.json")
    allp3 = mt.load(res, "p3-fcpq-h16-home-c*-nmean-*.json")
    rawc = load_raw(res, "p3-fcpq-h16-home-c*-nmean-w8-h8-b31-sus-*.json")
    fig, axs = plt.subplots(2, 3, figsize=(TWO, 2.35), layout="constrained")
    xs = list(range(len(CLAMPS)))
    xl = ["8", "16", "32", "off"]
    cl = [old[("sus", 8, 31, "fcpq-h16-home-c%d-nmean" % k)] for k in CLAMPS]
    for r in cl:
        assert r[0]["window"] == "09-29"
    fcr = ab[("sus", 8, 31, "fc-remote")]

    def model(runs):
        return mt.model_jain(med(runs, "lh") * med(runs, "cs_l") / med(runs, "cs_h"))

    def line(ax, key, color, marker, runs_list, scale=1.0, mfc=None, label=None):
        for x, runs in zip(xs, runs_list):
            a = agg(runs, key)
            ax.errorbar(x, a[0] * scale, yerr=yerr(a, scale), fmt=marker, color=color, mfc=mfc or color, ms=4, **ERR)
        ax.plot(xs, [med(r, key) * scale for r in runs_list], color=color, lw=0.8,
                label=label, marker=marker, mfc=mfc or color, ms=4)

    def clamp_axis(ax):
        ax.set_xticks(xs)
        ax.set_xticklabels(xl)
        ax.set_xlim(-0.4, len(xs) - 0.6)
        ax.set_xlabel("starvation clamp $c$ (passes)")

    # (a) pass limit, 09-28
    ax = axs[0, 0]
    hs = [("fc", "fc"), ("fcpq", "$H$=64"), ("fcpq-h8", "$H$=8"), ("fcpq-h16", "$H$=16"),
          ("fcpq-h16-home", "$H$=16\nhome")]
    for i, (v, _) in enumerate(hs):
        runs = old[("sus", 8, 31, v)]
        assert runs[0]["window"] == "09-28"
        a = agg(runs, "sj")
        ax.bar(i, a[0], 0.7, color=COL["fc"] if v == "fc" else PURPLE, yerr=yerr(a), error_kw=ERR, **EDGE)
        ax.plot(i, model(runs), marker="_", color="black", ms=9, mew=1.2)
    ax.axhline(0.95, ls="--", color="black", lw=0.6)
    ax.text(-0.45, 0.955, "G1", fontsize=7, va="bottom")
    ax.set_xticks(range(len(hs)))
    ax.set_xticklabels([h[1] for h in hs], fontsize=7)
    ax.set_ylim(0.6, 1.02)
    ax.set_ylabel("service Jain")
    ax.legend(handles=[Line2D([], [], ls="", marker="_", color="black", ms=9, mew=1.2, label="two-class model")],
              loc="upper left", bbox_to_anchor=(0.0, 0.88), fontsize=7)
    panel(ax, "a", "Pass limit $H$ (09-28)")

    # (b) service Jain vs clamp, with the confirmation cells
    ax = axs[0, 1]
    line(ax, "sj", PURPLE, "o", cl, label="W8 sus. b31")
    ax.plot(xs, [model(r) for r in cl], ls="", marker="_", color="black", ms=9, mew=1.2, zorder=6)
    for (cont, w, b), mk, lab in ((("sus", 8, 0), "s", "W8 sus. b0"), (("sus", 16, 31), "D", "W16 sus. b31"),
                                  (("bur", 8, 31), "v", "W8 bur. b31")):
        pts = [(x, allp3.get((cont, w, b, "fcpq-h16-home-c%d-nmean" % k))) for x, k in zip(xs, CLAMPS)]
        pts = [(x, r) for x, r in pts if r]
        ax.plot([x + 0.12 for x, _ in pts], [med(r, "sj") for _, r in pts], ls=":", lw=0.7,
                marker=mk, mfc="white", mec=PURPLE if cont == "sus" else GREY,
                color=PURPLE if cont == "sus" else GREY, ms=3.6, label=lab)
    ax.axhline(med(fcr, "sj"), ls="--", color=GREEN, lw=0.8)
    ax.text(-0.35, med(fcr, "sj") - 0.008, "fc-remote", color=GREEN, fontproperties=mono(7), ha="left", va="top")
    ax.axhline(0.95, ls="--", color="black", lw=0.6)
    ax.set_ylim(0.6, 1.02)
    clamp_axis(ax)
    ax.set_ylabel("service Jain")
    ax.legend(loc="lower right", fontsize=7, bbox_to_anchor=(1.0, 0.18), ncol=2)
    panel(ax, "b", "Clamp sweep (09-29)")

    # (c) L:H vs clamp with the ratios the model needs
    ax = axs[0, 2]
    line(ax, "lh", PURPLE, "o", cl)
    c16 = cl[1]
    csl, csh = med(c16, "cs_l"), med(c16, "cs_h")
    lh95 = mt.solve_x(0.95) * csh / csl
    ax.axhline(lh95, ls="--", color="black", lw=0.6)
    ax.axhline(csh / csl, ls=":", color="black", lw=0.8)
    ax.text(len(xs) - 0.65, lh95 - 0.1, "Jain 0.95 needs %.2f" % lh95, fontsize=7, ha="right", va="top")
    ax.text(len(xs) - 0.65, csh / csl + 0.1, "Jain 1 needs $C_H/C_L$ = %.2f" % (csh / csl), fontsize=7,
            ha="right", va="bottom")
    ax.annotate("%.2f: equal charged usage" % med(c16, "lh"), (1, med(c16, "lh")), xytext=(1.5, 5.05),
                textcoords="data", fontsize=7, ha="center", va="top",
                arrowprops=dict(arrowstyle="-", lw=0.5, color="black"))
    ax.set_ylim(3, 6.9)
    clamp_axis(ax)
    ax.set_ylabel("light : heavy ops")
    panel(ax, "c", "Light:heavy ops (09-29)")

    # (d) throughput
    ax = axs[1, 0]
    line(ax, "thr", PURPLE, "o", cl, scale=1e-6)
    ax.axhline(med(fcr, "thr") * 1e-6, ls="--", color=GREEN, lw=0.8)
    ax.text(len(xs) - 0.65, med(fcr, "thr") * 1e-6 + 0.006, "fc-remote", color=GREEN, fontproperties=mono(7),
            ha="right", va="bottom")
    ax.set_ylim(0.3, 0.66)
    clamp_axis(ax)
    ax.set_ylabel("throughput (Mops/s)")
    panel(ax, "d", "Throughput (09-29)")

    # (e) heavy-client latency
    ax = axs[1, 1]
    hmax = []
    for k in CLAMPS:
        runs = rawc[("sus", 8, 31, "fcpq-h16-home-c%d-nmean" % k)]
        v = sorted(mt.summarize.us(r["classes"]["heavy"]["run_latency"]["max"], r["tsc_hz"]) for r in runs)
        hmax.append((v[len(v) // 2], v[0], v[-1]))
    line(ax, "h_p50", PURPLE, "o", cl, label="p50")
    line(ax, "h_p99", PURPLE, "s", cl, mfc="white", label="p99")
    for x, a in zip(xs, hmax):
        ax.errorbar(x, a[0], yerr=yerr(a), fmt="^", color=BLACK, ms=4, **ERR)
    ax.plot(xs, [a[0] for a in hmax], color=BLACK, lw=0.8, marker="^", ms=4, label="max")
    ax.set_yscale("log")
    ax.set_ylim(150, 8000)
    ax.set_yticks([200, 500, 1000, 2000, 5000])
    ax.yaxis.set_major_formatter(ticker.FuncFormatter(lambda v, _: "%g" % v))
    ax.yaxis.set_minor_formatter(ticker.NullFormatter())
    clamp_axis(ax)
    ax.set_ylabel("heavy-client latency (µs)")
    ax.legend(loc="upper left", fontsize=7, ncol=3)
    panel(ax, "e", "Heavy-client latency (09-29)")

    # (f) worst queue wait
    ax = axs[1, 2]
    line(ax, "w_max", PURPLE, "o", cl)
    ax.plot(xs[:3], [k + 1 for k in CLAMPS[:3]], ls="", marker="_", color="black", ms=9, mew=1.0)
    ax.text(0.15, 9, "$c{+}1$", fontsize=7, va="center")
    ax.set_yscale("log")
    ax.set_ylim(5, 400)
    ax.set_yticks([10, 30, 100, 300])
    ax.yaxis.set_major_formatter(ticker.FuncFormatter(lambda v, _: "%g" % v))
    ax.yaxis.set_minor_formatter(ticker.NullFormatter())
    clamp_axis(ax)
    ax.set_ylabel("max. queue wait (passes)")
    panel(ax, "f", "Wait bound (09-29)")
    for a in axs.flat:
        a.grid(axis="y", which="minor", visible=False)
    save(fig, out, "fig-service.svg")


CO2_STYLE = {  # JSON label -> (printed name, colour, marker)
    "co-pq-sremote-k64-c256": ("co-pq", VERM, "o"),
    "dispatch-pq-home-c256": ("dispatch-pq-home-c256", ORANGE, "s"),
    "fcpq-h16-home-c16": ("fcpq-h16-home-c16", PURPLE, "D"),
    "dispatch": ("dispatch", GREY, "s"),
    "tokio-mutex": ("tokio-mutex", BLACK, "X"),
    "co-fifo-sremote-k64-home": ("co-fifo", SKY, "o"),
    "ces-k64-home": ("ces-k64-home", BLUE, "^"),
    "fc-remote": ("fc-remote", GREEN, "v"),
}


def fig_co2(res, out):
    """Per-op cost o against service fairness, W8 (a: sustained, b: bursty), spin filled / yield hollow;
    (c) o at W8 and W16 (sustained) for the fair locks and co-fifo. All 09-30 window."""
    c = mt.load_co2(res)
    fig, axs = plt.subplots(1, 3, figsize=(TWO, 2.55), layout="constrained",
                            gridspec_kw=dict(width_ratios=[1.0, 1.0, 0.9]))
    for ax, (cont, title, ylim) in zip(axs[:2], (("sus", "Sustained, 64 clients", (800, 9000)),
                                                 ("bur", "Bursty, 16 clients", (1000, 40000)))):
        for lab, (name, col, mk) in CO2_STYLE.items():
            for mode in ("spin", "yield"):
                runs = c[(cont, 8, mode, lab)]
                assert runs[0]["window"] == "09-30"
                j, o = agg(runs, "jain"), agg(runs, "o")
                ax.errorbar(j[0], o[0], xerr=yerr(j), yerr=yerr(o), fmt=mk, color=col,
                            mfc=col if mode == "spin" else "white", mec="black" if mode == "spin" else col,
                            mew=0.4 if mode == "spin" else 0.9, ms=4.6, **ERR)
        ax.set_yscale("log")
        ax.set_ylim(*ylim)
        ax.set_xlim(0.64, 1.02)
        ax.set_xticks([0.65, 0.7, 0.8, 0.9, 1.0])
        ax.set_xlabel("service Jain")
        ax.axvline(0.95, ls="--", color="black", lw=0.6)
        ax.set_yticks([1000, 2000, 5000] if cont == "sus" else [1000, 3000, 10000, 30000])
        ax.yaxis.set_major_formatter(ticker.FuncFormatter(lambda v, _: "%g" % v))
        ax.yaxis.set_minor_formatter(ticker.NullFormatter())
        ax.grid(axis="y", which="minor", visible=False)
        ax.grid(axis="x", visible=False)
    axs[0].set_ylabel("non-CS cycles per op, $o$")
    panel(axs[0], "a", "Sustained, 64 clients")
    panel(axs[1], "b", "Bursty, 16 clients")

    ax = axs[2]
    locks = ["co-pq-sremote-k64-c256", "dispatch-pq-home-c256", "fcpq-h16-home-c16", "co-fifo-sremote-k64-home"]
    wd = 0.36
    for i, lab in enumerate(locks):
        name, col, _ = CO2_STYLE[lab]
        for k, w in enumerate((8, 16)):
            runs = c[("sus", w, "spin", lab)]
            o = agg(runs, "o")
            ax.bar(i + (k - 0.5) * wd, o[0], wd, color=col, hatch=None if w == 8 else "////", yerr=yerr(o),
                   error_kw=ERR, **EDGE)
            oy = med(c[("sus", w, "yield", lab)], "o")
            ax.plot(i + (k - 0.5) * wd, oy, marker="D", mfc="white", mec="black", mew=0.7, ms=2.8, ls="", zorder=6)
    ax.set_xticks(range(len(locks)))
    ax.set_xticklabels([CO2_STYLE[l][0] for l in locks], fontproperties=mono(7), rotation=35, ha="right",
                       rotation_mode="anchor")
    ax.tick_params(axis="x", length=0)
    ax.set_xlim(-0.6, len(locks) - 0.4)
    ax.set_ylim(0, 7000)
    ax.set_ylabel("$o$, sustained")
    ax.legend(handles=[Patch(facecolor="#DDDDDD", label="$W$=8 (spin)", **EDGE),
                       Patch(facecolor="#DDDDDD", hatch="////", label="$W$=16 (spin)", **EDGE),
                       Line2D([], [], ls="", marker="D", mfc="white", mec="black", mew=0.7, ms=2.8, label="yield")],
              loc="upper right", fontsize=7, bbox_to_anchor=(1.02, 1.04))
    panel(ax, "c", "Scaling to $W$=16")

    h = [Line2D([], [], ls="", marker=mk, color=col, mec="black", mew=0.4, ms=4.6, label=name)
         for name, col, mk in CO2_STYLE.values()]
    h += [Line2D([], [], ls="", marker="o", color="#DDDDDD", mec="black", mew=0.4, ms=4.6, label="spin"),
          Line2D([], [], ls="", marker="o", mfc="white", mec="#555555", mew=0.9, ms=4.6, label="yield")]
    leg = fig.legend(handles=h, prop=mono(7), loc="outside lower center", ncol=5)
    for t in leg.get_texts()[-2:]:
        t.set_fontproperties(font_manager.FontProperties(family="serif", size=7))
    save(fig, out, "fig-co2.svg")



def main():
    here_paper = os.path.join(HERE, "..")
    res = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here_paper, "..", "results")
    out = sys.argv[2] if len(sys.argv) > 2 else os.path.join(here_paper, "figures")
    os.makedirs(out, exist_ok=True)
    setup_style()
    for f in (fig_motivation, fig_burden, fig_service, fig_co2):
        f(res, out)
    print("make_figures: wrote %s" % ", ".join(sorted(p for p in os.listdir(out) if p.startswith("fig-")
                                                     and p.endswith(".svg"))))


if __name__ == "__main__":
    main()
