#!/usr/bin/env python3
"""E0(b) FC-PQ fast-path ablation: build variants, run the matrix, summarize.

  runner.py build <variant>...           cargo build per feature set, keep binary
  runner.py run --tag T <variant>...     run cells (FC reference + FC-PQ each trial)
  runner.py summarize                    raw CSVs + markdown tables from all runs

Binaries: .worktree/bins/dlock-<slug>; arrow/stdout: .worktree/e0b/<tag>/<slug>/.
Raw per-trial/per-thread CSVs and the cell log go to .worktree/e0b/raw/ (ignored).
"""

import argparse
import csv
import glob
import hashlib
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
RAW = ROOT / ".worktree" / "e0b" / "raw"
TARGET_DIR = ROOT / ".worktree" / "bench-target"
BINS = ROOT / ".worktree" / "bins"
RUNS = ROOT / ".worktree" / "e0b"

VARIANTS = {
    "baseline": [],
    "cached_tid": ["fcpq_cached_tid"],
    "fast_path": ["fcpq_fast_path"],
    "fast_path+cached_tid": ["fcpq_fast_path", "fcpq_cached_tid"],
    "fast_path_notime": ["fcpq_fast_path_notime"],
    "fast_path+cached_tid+stat": [
        "fcpq_fast_path",
        "fcpq_cached_tid",
        "fcpq_fast_path_stat",
    ],
}

# cs / non-cs are loop iterations of the counter-proportional workload; a list
# is assigned to threads round-robin. cs=1000 is ~1.1k TSC cycles of hold time.
# The harness pins workers to core_affinity::get_core_ids(), i.e. the process
# affinity mask, in order; `taskset -c first_cpu..first_cpu+T-1` therefore
# places them on node 1 physical cores (node1 = CPUs 32-63 + SMT 96-127),
# away from CPU 0's IRQ load.
CELLS = {
    "a-tiny": dict(threads=[1], cs="1", noncs="0", trials=10, first_cpu=48),
    "a-1k": dict(threads=[1], cs="1000", noncs="0", trials=10, first_cpu=48),
    "b-lo": dict(threads=[2, 4, 8], cs="100", noncs="100000", first_cpu=32),
    "b-mid": dict(threads=[2, 4, 8], cs="100", noncs="10000", first_cpu=32),
    "c-cs1": dict(threads=[32], cs="1", noncs="0", first_cpu=32),
    "c-cs1k": dict(threads=[32], cs="1000", noncs="0", first_cpu=32),
    "d-het": dict(threads=[8, 32], cs="1000,8000", noncs="0", first_cpu=32),
}

LOCKS = "fc,fc-pq-b-heap"
LOCK_NAMES = {"FC": "FC", "FC_PQ_BHeap": "FC-PQ"}


def slug(variant):
    return variant.replace("+", "-")


def source_digest():
    h = hashlib.sha256()
    files = sorted(
        glob.glob(str(ROOT / "crates/libdlock/src/**/*.rs"), recursive=True)
        + glob.glob(str(ROOT / "src/**/*.rs"), recursive=True)
        + [str(ROOT / "Cargo.toml"), str(ROOT / "crates/libdlock/Cargo.toml")]
    )
    for f in files:
        h.update(f.encode())
        h.update(Path(f).read_bytes())
    return h.hexdigest()[:16]


def build(variant):
    feats = VARIANTS[variant]
    cmd = ["cargo", "build", "--release", "-p", "dlock"]
    if feats:
        cmd += ["--features", ",".join(feats)]
    print("+", " ".join(cmd), flush=True)
    env = dict(os.environ, CARGO_TARGET_DIR=str(TARGET_DIR))
    BINS.mkdir(parents=True, exist_ok=True)
    with open(BINS / f"build-{slug(variant)}.log", "w") as log:
        subprocess.run(cmd, cwd=ROOT, env=env, check=True, stdout=log, stderr=log)
    dest = BINS / f"dlock-{slug(variant)}"
    shutil.copy2(TARGET_DIR / "release" / "dlock", dest)
    commit = subprocess.run(
        ["jj", "log", "-r", "@", "--no-graph", "-T", "commit_id.short()"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    ).stdout.strip()
    meta = dict(
        variant=variant,
        features=feats,
        command=" ".join(cmd),
        jj_commit=commit,
        source_sha256_16=source_digest(),
        built=time.strftime("%Y-%m-%dT%H:%M:%S%z"),
    )
    (BINS / f"dlock-{slug(variant)}.json").write_text(json.dumps(meta, indent=1))
    print(json.dumps(meta))


def uptime():
    return subprocess.run(["uptime"], capture_output=True, text=True).stdout.strip()


def run(variant, tag, cells, trials, duration, warmup, only_threads=None):
    binary = BINS / f"dlock-{slug(variant)}"
    meta = json.loads((BINS / f"dlock-{slug(variant)}.json").read_text())
    RAW.mkdir(parents=True, exist_ok=True)
    for cell in cells:
        spec = CELLS[cell]
        # Explicit --trials wins; otherwise the cell default (10 for 1W), else 5.
        cell_trials = trials or spec.get("trials", 5)
        for t in spec["threads"]:
            if only_threads and t not in only_threads:
                continue
            out = RUNS / tag / slug(variant) / cell / f"t{t}"
            if out.exists():
                shutil.rmtree(out)
            out.mkdir(parents=True)
            cmd = [
                "taskset", "-c", f"{spec['first_cpu']}-{spec['first_cpu'] + t - 1}",
                str(binary), "d-lock2", "-t", str(t), "-d", str(duration),
                "--warmup", str(warmup), "--trials", str(cell_trials), "-o", str(out),
                "-l", LOCKS, "counter-proportional", "--cs", spec["cs"],
                "--non-cs", spec["noncs"], "--file-name", cell,
            ]
            before = uptime()
            start = time.time()
            res = subprocess.run(cmd, capture_output=True, text=True)
            (out / "stdout.txt").write_text(res.stdout + res.stderr)
            entry = dict(
                tag=tag, variant=variant, cell=cell, threads=t, cs=spec["cs"],
                noncs=spec["noncs"], trials=cell_trials, duration=duration,
                warmup=warmup, uptime_before=before, uptime_after=uptime(),
                wall_s=round(time.time() - start, 1), returncode=res.returncode,
                binary_source_sha256_16=meta["source_sha256_16"],
                command=" ".join(cmd).replace(str(ROOT) + "/", ""),
            )
            with open(RAW / "cells.jsonl", "a") as f:
                f.write(json.dumps(entry) + "\n")
            print(f"{tag} {variant} {cell} t{t} rc={res.returncode} | {before}", flush=True)
            if res.returncode != 0:
                sys.exit(res.stdout[-2000:] + res.stderr[-2000:])


HITS_RE = re.compile(r"Fast path hits: (\d+) acquisitions: (\d+) \(trial (\d+)\)")


def load_rows():
    import pyarrow.ipc as ipc

    trials, threads = [], []
    for stdout in sorted(RUNS.glob("*/*/*/t*/stdout.txt")):
        d = stdout.parent
        tag, vslug, cell = d.parts[-4], d.parts[-3], d.parts[-2]
        variant = next(v for v in VARIANTS if slug(v) == vslug)
        hits = {int(m[3]): (int(m[1]), int(m[2])) for m in HITS_RE.finditer(stdout.read_text())}
        for f in sorted(d.glob("*/*.arrow")):
            lock = LOCK_NAMES.get(f.parent.name, f.parent.name)
            by_trial = {}
            for r in ipc.open_file(f).read_all().to_pylist():
                by_trial.setdefault(r["trial"], []).append(r)
                threads.append(dict(
                    tag=tag, variant=variant, cell=cell, lock=lock,
                    threads=r["thread_num"], trial=r["trial"], id=r["id"],
                    cpu=r["cpu_id"], cs=r["cs_length"], noncs=r["non_cs_length"],
                    num_acquire=r["num_acquire"], hold_time=r["hold_time"],
                    combine_time=r["combine_time"], normalized_share=r["normalized_share"],
                ))
            for k, rs in sorted(by_trial.items()):
                dur = rs[0]["duration"]
                acq = sum(r["num_acquire"] for r in rs)
                h = hits.get(k) if lock == "FC-PQ" else None
                trials.append(dict(
                    tag=tag, variant=variant, cell=cell, lock=lock,
                    threads=rs[0]["thread_num"], trial=k, duration=dur,
                    mops=acq / dur / 1e6, jain=rs[0]["jfi"],
                    work_mloops=sum(r["loop_count"] for r in rs) / dur / 1e6,
                    hold_per_acq=sum(r["hold_time"] for r in rs) / max(acq, 1),
                    fp_hits=h[0] if h else "", fp_acquisitions=h[1] if h else "",
                    fp_hit_rate=h[0] / h[1] if h and h[1] else "",
                ))
    return trials, threads


def write_csv(path, rows):
    with open(path, "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=list(rows[0]))
        w.writeheader()
        w.writerows(rows)


def fmt(xs, p=3):
    if not xs:
        return "-"
    return f"{statistics.median(xs):.{p}f} [{min(xs):.{p}f}, {max(xs):.{p}f}]"


def summarize():
    trials, threads = load_rows()
    RAW.mkdir(parents=True, exist_ok=True)
    write_csv(RAW / "trials.csv", trials)
    write_csv(RAW / "threads.csv", threads)

    key = lambda r: (r["tag"], r["variant"], r["cell"], r["threads"])
    groups = {}
    for r in trials:
        groups.setdefault(key(r), {}).setdefault(r["lock"], {})[r["trial"]] = r
    cells_order = list(CELLS)
    var_order = list(VARIANTS)
    lines = [
        "| tag | cell | T | variant | FC-PQ Mops/s | FC Mops/s | paired FC-PQ/FC | FC-PQ ns/op | FC ns/op | FC-PQ Jain | FC Jain | hit rate |",
        "|---|---|---:|---|---|---|---|---|---|---|---|---|",
    ]
    for k in sorted(groups, key=lambda k: (cells_order.index(k[2]), k[3], k[0], var_order.index(k[1]))):
        g = groups[k]
        pq, fc = g.get("FC-PQ", {}), g.get("FC", {})
        ratios = [pq[t]["mops"] / fc[t]["mops"] for t in pq if t in fc]
        hr = [pq[t]["fp_hit_rate"] for t in pq if pq[t]["fp_hit_rate"] != ""]
        lines.append(
            f"| {k[0]} | {k[2]} | {k[3]} | {k[1]} | {fmt([r['mops'] for r in pq.values()])} | "
            f"{fmt([r['mops'] for r in fc.values()])} | {fmt(ratios)} | "
            f"{fmt([1e3 / r['mops'] for r in pq.values()], 1)} | {fmt([1e3 / r['mops'] for r in fc.values()], 1)} | "
            f"{fmt([r['jain'] for r in pq.values()], 4)} | {fmt([r['jain'] for r in fc.values()], 4)} | "
            f"{fmt(hr, 4)} |"
        )
    table = "\n".join(lines)
    (RAW / "summary.md").write_text(table + "\n")
    print(table)


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build")
    b.add_argument("variants", nargs="+", choices=list(VARIANTS))
    r = sub.add_parser("run")
    r.add_argument("variants", nargs="+", choices=list(VARIANTS))
    r.add_argument("--tag", required=True)
    r.add_argument("--cells", default=",".join(CELLS))
    r.add_argument("--trials", type=int, default=None)
    r.add_argument("--threads", default="", help="comma list; subset of each cell's thread counts")
    r.add_argument("--duration", type=int, default=5)
    r.add_argument("--warmup", type=int, default=2)
    sub.add_parser("summarize")
    a = ap.parse_args()
    if a.cmd == "build":
        for v in a.variants:
            build(v)
    elif a.cmd == "run":
        for v in a.variants:
            only = [int(x) for x in a.threads.split(",") if x]
            run(v, a.tag, a.cells.split(","), a.trials, a.duration, a.warmup, only)
    else:
        summarize()


if __name__ == "__main__":
    main()
