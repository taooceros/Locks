#!/usr/bin/env python3
"""Generate the paper's data figures (vector SVG) from the result JSONs.

usage: make_figures.py [RESULTS_DIR] [OUT_DIR]     (defaults: ../results  figures)

Every figure is built with the loaders and metric definitions of
make_tables.py (which imports summarize.py and summarize_fcpq_sweep.py), so a
figure and a table built from the same cell show the same numbers. A bar or
marker is the median over the repeats of one configuration; error bars span
[min, max] over the repeats. Ratios divide by the median of the reference.

Measurement windows: make_tables.load() refuses to aggregate a cell that
mixes the 09-28 (turbo) and 09-29 (3.0 GHz cap) windows. Where one axis
shows cells from both windows, 09-29 cells are drawn as hollow markers;
panels that hold a single window name it in their title.

Output: fig-motivation.svg, fig-burden.svg, fig-service.svg, fig-xrt.svg,
fig-actor.svg, fig-util.svg. Widths: one column 3.3 in, two columns 7 in; the
paper includes them at that width, so the 7.5-8 pt text is printed at size.
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
    """Both subversions: (a,b) combiner burden at W8 sus b0; (c) FIFO service share."""
    raw = load_raw(res, "p3-*-w8-h8-b0-sus-*.json")
    met = mt.load(res, "p3-*-w8-h8-b0-sus-*.json")
    fig = plt.figure(figsize=(TWO, 2.0), layout="constrained")
    gs = fig.add_gridspec(1, 3, width_ratios=[1.35, 0.75, 1.9])

    # (a) per-worker share of combining cycles, workers sorted by share
    ax = fig.add_subplot(gs[0])
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
    ax.text(8.45, 1 / 8 + 0.02, "fair", ha="right", va="bottom", fontsize=7)
    ax.set_xticks(range(1, 9))
    ax.set_xlim(0.45, 8.55)
    ax.set_ylim(0, 1.04)
    ax.set_xlabel("worker, by combining share")
    ax.set_ylabel("share of combining cycles")
    ax.legend(prop=mono(7), loc="upper right", title="burden Jain", title_fontsize=7)
    panel(ax, "a", "Combiner burden")

    # (b) bystander p99 on the combiner worker vs. the other workers
    ax = fig.add_subplot(gs[1])
    top = 8e3
    for i, v in enumerate(variants):
        runs = met[("sus", 8, 0, v)]
        c, o = agg(runs, "comb_p99"), agg(runs, "noncomb_p99")
        ax.errorbar(i + 0.12, o[0], yerr=yerr(o), fmt="o", mfc="white", mec=COL[v], ms=4.5, **ERR)
        if c[0] > mt.CENSORED_US:
            ax.plot(i - 0.12, top, marker="^", color=COL[v], ms=5)
            ax.text(i + 0.12, top, "never polled", fontsize=7, va="center", ha="left")
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
    panel(ax, "b", "Its bystander")

    # (c) per-client service under FIFO vs. usage ordering (W8 sus b31)
    ax = fig.add_subplot(gs[2])
    series = [("dispatch", load_raw(res, "p3-dispatch-w8-h8-b31-sus-*.json")[("sus", 8, 31, "dispatch")],
               dict(marker="o", color=GREY, mfc=GREY, ms=2.4), -0.22),
              ("tokio-mutex", load_raw(res, "tokio-tokio-mutex-w8-h8-sus-*.json")[("sus", 8, None, "tokio-mutex")],
               dict(marker="x", color=BLACK, mfc=BLACK, ms=2.6), 0.22),
              ("fcpq-h16-home-c16", load_raw(res, "p3-fcpq-h16-home-c16-nmean-w8-h8-b31-sus-*.json")[
                  ("sus", 8, 31, "fcpq-h16-home-c16-nmean")], dict(marker="^", color=PURPLE, mfc="white", ms=2.6),
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
    ax.text(15.5, lo - 0.07, "FIFO theory: $2C_L/(C_L+C_H)$", ha="center", va="top", fontsize=7)
    ax.text(63.3, hi - 0.07, "FIFO theory: $2C_H/(C_L+C_H)$", ha="right", va="top", fontsize=7)
    ax.text(47.5, 0.97, "equal lock time", ha="center", va="top", fontsize=7)
    ax.text(63.3, 0.5, "hollow: 09-29 window\n(3.0 GHz cap)", ha="right", va="center", fontsize=7)
    ax.set_xlim(-1, 64)
    ax.set_ylim(0, 3.0)
    ax.set_yticks([0, 0.5, 1, 1.5, 2, 2.5])
    ax.set_xticks([15.5, 47.5])
    ax.set_xticklabels(["32 light clients (CS 1×)", "32 heavy clients (CS 8×)"])
    ax.tick_params(axis="x", length=0)
    ax.set_ylabel("client CS time / mean")
    ax.legend(handles=handles, prop=mono(7), loc="upper left", ncol=1, title="service Jain", title_fontsize=7)
    panel(ax, "c", "Service unfairness (W8, sus., b31)")
    save(fig, out, "fig-motivation.svg")


BURDEN_VARIANTS = ("ces", "ces-k64", "ces-k64-home", "ces-t64000-home", "fc", "fc-home", "fc-remote")
CELLS4 = (("sus", 0), ("sus", 31), ("bur", 0), ("bur", 31))
CELL_NAME = {("sus", 0): "sustained, b0", ("sus", 31): "sustained, b31",
             ("bur", 0): "bursty, b0", ("bur", 31): "bursty, b31"}


def fig_burden(res, out):
    """Burden Jain and combiner-worker bystander p99, variants x cells, W8 (+W16)."""
    c = mt.load(res, "p3-*-h8-*.json")
    fig, axs = plt.subplots(2, 4, figsize=(TWO, 3.0), layout="constrained", sharex=True, sharey="row",
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
    axs[0, 0].text(-0.5, 0.91, "G1", ha="left", va="bottom", fontsize=7)
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
    fig, axs = plt.subplots(2, 3, figsize=(TWO, 3.05), layout="constrained")
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
    ax.text(-0.45, 0.955, "G2", fontsize=7, va="bottom")
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
    ax.legend(loc="lower right", fontsize=7, bbox_to_anchor=(1.0, 0.25))
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


XRT_BARS = [("tokio", "tokio-mutex-lifo-off", "tokio-mutex\nLIFO off"), ("tokio", "async-lock", "async-lock"),
            ("tokio", "std-mutex", "std-mutex"), ("tokio", "parking-lot", "parking-lot"),
            ("coro", "dispatch", "dispatch"), ("coro", "ces-k64-home", "ces-k64-home"),
            ("coro", "fc-remote", "fc-remote"), ("coro", "fcpq-h16-home", "fcpq-h16-home")]


def fig_xrt(res, out):
    """Throughput relative to tokio::sync::Mutex (LIFO on), and to LIFO off."""
    tk = mt.load(res, "tokio-*.json")
    co = mt.load(res, "xrt-*.json")
    lifo = mt.load_lifo(res)
    fig, ax = plt.subplots(figsize=(TWO, 2.25), layout="constrained")
    nb = len(XRT_BARS)
    wd = 0.8 / nb
    colors = {"tokio-mutex-lifo-off": "#FFFFFF", **COL}
    for g, (w, cont) in enumerate(mt.XRT_CELLS):
        ref = med(tk[(cont, w, None, "tokio-mutex")], "thr")
        off = med(lifo[(w, cont, "tokio-mutex", "off")], "thr")
        for i, (rt, lock, _) in enumerate(XRT_BARS):
            x = g + (i - (nb - 1) / 2) * wd
            if lock == "tokio-mutex-lifo-off":
                # within the throwaway LIFO build: off / on
                on_ = med(lifo[(w, cont, "tokio-mutex", "on")], "thr")
                a = agg(lifo[(w, cont, "tokio-mutex", "off")], "thr")
                a, runs = tuple(v / on_ for v in a), None
            else:
                runs = (tk if rt == "tokio" else co)[(cont, w, None if rt == "tokio" else 31, lock)]
                assert runs[0]["window"] == "09-28"
                a = tuple(v / ref for v in agg(runs, "thr"))
            ax.bar(x, a[0], wd, color=colors[lock], yerr=yerr(a), error_kw=ERR,
                   hatch="xxxx" if lock == "tokio-mutex-lifo-off" else None, **EDGE)
            if rt == "coro" and lock != "dispatch":
                ax.plot(x, med(runs, "thr") / off, marker="_", color="white", ms=7.5, mew=2.6)
                ax.plot(x, med(runs, "thr") / off, marker="_", color="black", ms=7, mew=1.1)
            if runs is not None and med(runs, "starved_c"):
                ax.text(x, a[2] * 1.07, "%d" % med(runs, "starved_c"), ha="center", va="bottom", fontsize=7,
                        color=VERM)
    ax.axhline(1, color="black", lw=0.8)
    ax.set_yscale("log")
    ax.set_ylim(0.15, 9)
    ax.set_yticks([0.2, 0.5, 1, 2, 5])
    ax.set_yticklabels(["0.2", "0.5", "1", "2", "5"])
    ax.yaxis.set_minor_formatter(ticker.NullFormatter())
    ax.grid(axis="y", which="minor", visible=False)
    ax.set_xticks(range(4))
    ax.set_xticklabels(["$W$=%d, %s" % (w, {"sus": "sustained", "bur": "bursty"}[c]) for w, c in mt.XRT_CELLS])
    ax.tick_params(axis="x", length=0)
    ax.set_xlim(-0.5, 3.5)
    ax.set_ylabel("throughput / tokio-mutex")
    for g in range(1, 4):
        ax.axvline(g - 0.5, color="#BBBBBB", lw=0.5)
    handles = [Patch(facecolor=colors[l], hatch="xxxx" if l == "tokio-mutex-lifo-off" else None,
                     label=lab.replace("\n", " "), **EDGE) for _, l, lab in XRT_BARS]
    handles.append(Line2D([], [], ls="", marker="_", color="black", ms=7, mew=1.1, label="÷ tokio-mutex LIFO off"))
    handles.append(Line2D([], [], ls="", marker="$63$", color=VERM, ms=7, mew=0.1, label="starved clients"))
    leg = fig.legend(handles=handles, prop=mono(7), loc="outside upper center", ncol=5)
    leg.get_texts()[-1].set_fontproperties(font_manager.FontProperties(family="serif", size=7))
    save(fig, out, "fig-xrt.svg")


def fig_actor(res, out):
    """Actor control: throughput vs. fc-remote against burden Jain."""
    ref = mt.load(res, "actorref-*.json")
    p3 = mt.load(res, "p3-*-h8-*.json")
    xrt = mt.load(res, "xrt-*.json")
    fig, axs = plt.subplots(1, 2, figsize=(ONE, 2.3), layout="constrained", sharey=True)
    cellmk = {(8, 31): "o", (8, 0): "s", (16, 31): "D"}
    for ax, cont in zip(axs, ("sus", "bur")):
        ax.add_patch(plt.Rectangle((0.9, 0.95), 0.15, 0.3, color="#E8F4EC", zorder=0, lw=0))
        for (w, b), mk in cellmk.items():
            if (w, b) == (8, 31):
                base_runs = ref[(cont, 8, 31, "fc-remote")]
                cands = [("ces-k64-home", ref[(cont, 8, 31, "ces-k64-home")])]
            else:
                base_runs = p3.get((cont, w, b, "fc-remote")) or xrt[(cont, w, b, "fc-remote")]
                cands = [("ces-k64-home", p3[(cont, w, b, "ces-k64-home")])]
            cands += [("actor", p3[(cont, w, b, "actor")]), ("actor-inline", p3[(cont, w, b, "actor-inline")])]
            base, bwin = med(base_runs, "thr"), base_runs[0]["window"]
            ax.plot(med(base_runs, "bj"), 1.0, marker=mk, color=COL["fc-remote"], ms=4.2, ls="")
            for v, runs in cands:
                x, y = agg(runs, "bj"), med(runs, "thr") / base
                cross = runs[0]["window"] != bwin
                ax.errorbar(x[0], y, xerr=yerr(x), fmt=mk, color=COL[v], mec=COL[v] if cross else "black",
                            mfc="white" if cross else COL[v], ms=4.2, mew=0.9 if cross else 0.4, **ERR)
        ax.set_xlim(-0.02, 1.06)
        ax.set_ylim(0, 1.2)
        ax.set_xticks([0, 0.5, 1])
        ax.set_xticklabels(["0", "0.5", "1"])
        ax.set_xticks([0.125], minor=True)
        ax.grid(axis="x", which="minor", visible=True, ls=":", color="#BBBBBB", lw=0.5)
        ax.set_xlabel("burden Jain")
        ax.set_title({"sus": "sustained", "bur": "bursty"}[cont])
        ax.text(1.04, 1.17, "goal", fontsize=7, ha="right", va="top", color=GREEN)
        ax.text(0.14, 0.03, "$1/W$", fontsize=7, ha="left", va="bottom", color="#777777")
    axs[0].set_ylabel("throughput / fc-remote")
    h = [Line2D([], [], ls="", marker="o", color=COL[v], mec="black", mew=0.4, ms=4.2, label=v)
         for v in ("fc-remote", "ces-k64-home", "actor", "actor-inline")]
    h2 = [Line2D([], [], ls="", marker=mk, color="#DDDDDD", mec="black", mew=0.4, ms=4, label=lab)
          for mk, lab in (("o", "W8 b31"), ("s", "W8 b0"), ("D", "W16 b31"))]
    h2.append(Line2D([], [], ls="", marker="o", mfc="white", mec="black", mew=0.9, ms=4,
                     label="hollow: 09-29 run ÷ 09-28 reference"))
    fig.legend(handles=h, prop=mono(7), loc="outside upper center", ncol=2)
    fig.legend(handles=h2, loc="outside lower center", ncol=3, fontsize=7)
    save(fig, out, "fig-actor.svg")


def fig_util(res, out):
    """Utilisation identity util = C / (C + o) with the measured FC-family points."""
    old = mt.load(res, "p3-*-w8-h8-b31-sus-*.json")
    ab = mt.load(res, "p3ab-pre-*-w8-h8-b31-sus-*.json")
    fig, ax = plt.subplots(figsize=(ONE, 2.45), layout="constrained")
    c16 = old[("sus", 8, 31, "fcpq-h16-home-c16-nmean")]
    csl, csh = med(c16, "cs_l"), med(c16, "cs_h")
    lh95 = mt.solve_x(0.95) * csh / csl
    cbar95 = (lh95 * csl + csh) / (lh95 + 1)
    o_need = cbar95 * (1 / 0.8 - 1)
    fcr = ab[("sus", 8, 31, "fc-remote")]
    ax.set_xlim(400, 1500)
    ax.set_ylim(0.6, 0.96)
    ax.set_xlabel("non-CS cycles per op, $o$")
    ax.set_ylabel("lock utilisation (CS / window)")
    curves = []
    for cb, ls, lab, xl, above in ((med(fcr, "cbar"), "-", "FIFO mix", 1260, True),
                                   (cbar95, "--", "Jain 0.95", 600, True),
                                   (med(c16, "cbar"), ":", "Jain 0.997", 1350, False)):
        xs = [x * 10 for x in range(40, 151)]
        ax.plot(xs, [cb / (cb + o) for o in xs], ls=ls, color="black", lw=0.8)
        curves.append((cb, lab, xl, above))
    # 09-28 points: filled; 09-29: hollow. Purple = FC-PQ, labelled by setting.
    pts = [(v, old[("sus", 8, 31, v)], v) for v in ("fc", "fc-home", "fc-remote")]
    pts += [("fcpq", old[("sus", 8, 31, v)], lab) for v, lab in
            (("fcpq", "$H$=64"), ("fcpq-h8", "$H$=8"), ("fcpq-h16", "$H$=16"), ("fcpq-h16-home", "$H$=16 home"))]
    pts += [("fc-remote", fcr, None), ("fcpq", old[("sus", 8, 31, "fcpq-h16-home-c8-nmean")], "clamp 8")]
    pts += [("fcpq", old[("sus", 8, 31, "fcpq-h16-home-c%d-nmean" % k)], None) for k in CLAMPS[1:]]
    lab_off = {"$H$=64": (-5, 0, "right"), "$H$=8": (5, 0, "left"), "$H$=16": (5, -3, "left"),
               "$H$=16 home": (4, 4, "left"), "clamp 8": (5, 2, "left")}
    for v, runs, lab in pts:
        o, u = agg(runs, "o"), agg(runs, "util")
        w29 = runs[0]["window"] == "09-29"
        col = COL[v]
        ax.errorbar(o[0], u[0], xerr=yerr(o), yerr=yerr(u), fmt="o", color=col, mfc="white" if w29 else col,
                    mec=col if w29 else "black", mew=0.9 if w29 else 0.4, ms=3.8, **ERR)
        if lab in lab_off:
            dx, dy, ha = lab_off[lab]
            ax.annotate(lab, (o[0], u[0]), xytext=(dx, dy), textcoords="offset points", fontsize=7,
                        ha=ha, va="center")
    # clamp 16 / 32 / off coincide; one label for the group
    grp = [old[("sus", 8, 31, "fcpq-h16-home-c%d-nmean" % k)] for k in CLAMPS[1:]]
    ax.annotate("clamp 16/32/off", (sum(med(r, "o") for r in grp) / 3, sum(med(r, "util") for r in grp) / 3),
                xytext=(-4, -7), textcoords="offset points", fontsize=7, ha="right", va="center")
    ax.plot([o_need, o_need], [0.5, 0.8], ls="-", color=VERM, lw=0.7)
    ax.plot([400, o_need], [0.8, 0.8], ls="-", color=VERM, lw=0.7)
    ax.plot(o_need, 0.8, marker="*", color=VERM, ms=6)
    ax.text(o_need - 12, 0.608, "$o \\leq %.0f$ for\nutil 0.80 at\nJain 0.95" % o_need, color=VERM, fontsize=7,
            ha="right", va="bottom")
    h = [Line2D([], [], ls="", marker="o", color=COL[v], mec="black", mew=0.4, ms=3.8, label=v)
         for v in ("fc", "fc-home", "fc-remote")]
    h += [Line2D([], [], ls="", marker="o", color=PURPLE, mec="black", mew=0.4, ms=3.8, label="fcpq"),
          Line2D([], [], ls="", marker="o", color=GREY, mec="black", mew=0.4, ms=3.8, label="09-28"),
          Line2D([], [], ls="", marker="o", color=GREY, mfc="white", mew=0.9, ms=3.8, label="09-29 (3 GHz cap)")]
    leg = ax.legend(handles=h, loc="upper right", ncol=3, prop=mono(7))
    for t in leg.get_texts()[4:]:
        t.set_fontproperties(font_manager.FontProperties(family="serif", size=7))
    fig.canvas.draw()  # final axes geometry, so the curve labels follow the drawn slope
    for cb, lab, xl, above in curves:
        f = (lambda o, cb=cb: cb / (cb + o))
        p0, p1 = ax.transData.transform((xl - 20, f(xl - 20))), ax.transData.transform((xl + 20, f(xl + 20)))
        ang = math.degrees(math.atan2(p1[1] - p0[1], p1[0] - p0[0]))
        ax.annotate("%s, $\\bar{C}$=%.0f" % (lab, cb), (xl, f(xl)), xytext=(0, 2 if above else -2),
                    textcoords="offset points", rotation=ang, rotation_mode="anchor", fontsize=7,
                    ha="center", va="bottom" if above else "top")
    save(fig, out, "fig-util.svg")


def main():
    here_paper = os.path.join(HERE, "..")
    res = sys.argv[1] if len(sys.argv) > 1 else os.path.join(here_paper, "..", "results")
    out = sys.argv[2] if len(sys.argv) > 2 else os.path.join(here_paper, "figures")
    os.makedirs(out, exist_ok=True)
    setup_style()
    for f in (fig_motivation, fig_burden, fig_service, fig_xrt, fig_actor, fig_util):
        f(res, out)
    print("make_figures: wrote %s" % ", ".join(sorted(p for p in os.listdir(out) if p.startswith("fig-")
                                                     and p.endswith(".svg"))))


if __name__ == "__main__":
    main()
