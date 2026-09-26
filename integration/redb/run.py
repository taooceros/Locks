#!/usr/bin/env python3
"""Fresh-process redb write-path trials; one immutable trial directory per cell.

--prepare-only snapshots binaries from a verified build (build.py) and freezes
their source/patch/binary identity and CPU/NUMA placement. --smoke runs a small
timed cohort (all variants, 3 repetitions) whose analysis includes the
refactored-vs-native control check. --run runs the formal matrix only when
explicitly requested. Every failure is retained.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import resource
import shutil
import statistics
import subprocess
import sys

from integration.redb.build import OUT, load_build

HERE = Path(__file__).resolve().parents[2]
VARIANTS = ('native', 'refactored', 'bridge_mutex', 'mcs', 'fc', 'fc_pq')
COHORTS = ('all1', 'half1_half8', 'half1_half64')
SMOKE_COHORTS = ('all1', 'half1_half64')
DURABILITIES = ('immediate', 'none')
DEFAULT_CPUS = tuple(range(8, 16))
SEEDS = (0x7BEF523C1678F92D, 0xDB3102F89158C44B, 0x7E3BB4A250D1E66F)
DURATION_MS = 2000
TRIAL_TIMEOUT_SECONDS = 30
DB_FILE_LIMIT = 512 * 1024 * 1024
MAX_RECORDS_PER_WORKER = 8_000_000
MAX_RECORDS = len(DEFAULT_CPUS) * MAX_RECORDS_PER_WORKER


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def write_json(path, data):
    path.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n")


def parse_cpus(value):
    try:
        cpus = tuple(int(cpu) for cpu in value.split(","))
    except ValueError as error:
        raise argparse.ArgumentTypeError("CPUs must be comma-separated integers") from error
    if len(cpus) != 8 or len(set(cpus)) != 8 or any(cpu < 0 for cpu in cpus):
        raise argparse.ArgumentTypeError("exactly eight distinct nonnegative CPUs are required")
    return cpus


def parse_numa_node(value):
    if value == "none":
        return None
    try:
        node = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("NUMA node must be a nonnegative integer or none") from error
    if node < 0:
        raise argparse.ArgumentTypeError("NUMA node must be a nonnegative integer or none")
    return node


def check_affinity(cpus):
    if os.sched_getaffinity(0) != set(cpus):
        raise RuntimeError(f"run under taskset -c {','.join(map(str, cpus))} (exactly eight requested CPUs)")


def command_capture(command, directory, timeout=None, limit_db=False):
    directory.mkdir(parents=True, exist_ok=False)
    (directory / "command.json").write_text(json.dumps(command) + "\n")
    def limits():
        if limit_db:
            resource.setrlimit(resource.RLIMIT_FSIZE, (DB_FILE_LIMIT, DB_FILE_LIMIT))
    try:
        completed = subprocess.run(command, cwd=HERE, timeout=timeout,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   preexec_fn=limits if limit_db else None, check=False)
        out, err, exit_code, timed_out = completed.stdout, completed.stderr, completed.returncode, False
    except subprocess.TimeoutExpired as failure:
        out, err, exit_code, timed_out = failure.stdout or b"", failure.stderr or b"", None, True
    except OSError as failure:
        out, err, exit_code, timed_out = b"", str(failure).encode(), None, False
    (directory / "stdout").write_bytes(out)
    (directory / "stderr").write_bytes(err)
    return {"command": command, "exit_code": exit_code, "timed_out": timed_out,
            "timeout_seconds": timeout}


def placement(cpus, numa_node):
    """CPU topology rows for the chosen CPUs and the memory policy actually applied."""
    rows = subprocess.check_output(["lscpu", "-p=CPU,CORE,SOCKET,NODE"], text=True).splitlines()
    topology = {}
    for row in rows:
        if row.startswith("#"):
            continue
        cpu, core, socket, node = (int(x) if x else None for x in row.split(","))
        if cpu in cpus:
            topology[cpu] = {"core": core, "socket": socket, "node": node}
    if len(topology) != len(cpus):
        raise RuntimeError("some requested CPUs are not present in lscpu output")
    model = next((line.split(":", 1)[1].strip() for line in
                  subprocess.check_output(["lscpu"], text=True).splitlines()
                  if line.startswith("Model name")), None)
    policy = None
    if numa_node is not None:
        policy = subprocess.check_output(["numactl", f"--membind={numa_node}", "numactl", "--show"],
                                         text=True).splitlines()
    return {"cpus": list(cpus), "topology": topology,
            "distinct_physical_cores": len({(t["socket"], t["core"]) for t in topology.values()}),
            "cpu_nodes": sorted({t["node"] for t in topology.values()}),
            "numa_memory_node": numa_node, "numactl_show_under_membind": policy,
            "cpu_model": model, "kernel": platform.release()}


def prepare(root, build_dir, cpus, numa_node):
    check_affinity(cpus)
    build = load_build(build_dir)
    root.mkdir(parents=True, exist_ok=False)
    binaries = root / "binaries"
    binaries.mkdir()
    snapshots = {}
    for name, entry in build["binaries"].items():
        if name == "test_hooks":
            continue  # correctness-gate only; never timed
        snapshot = binaries / name
        shutil.copy2(build_dir / entry["file"], snapshot)
        snapshot.chmod(0o555)
        if digest(snapshot) != entry["sha256"]:
            raise RuntimeError(f"snapshot of {name} differs from build manifest")
        snapshots[name] = {"file": str(snapshot.relative_to(root)), "sha256": entry["sha256"],
                           "features": entry["features"], "build_command": entry["command"]}
    identity = {
        "schema": 2,
        "build_dir": str(build_dir),
        "redb_source": build["redb_source"],
        "harness_sources_sha256": build["harness_sources_sha256"],
        "runner_sha256": digest(Path(__file__).resolve()),
        "build_git_head": build["git_head"], "build_git_dirty_paths": build["git_dirty_paths"],
        "git_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=HERE, text=True).strip(),
        "rustc": build["rustc"], "binaries": snapshots, "variant_binary": build["variant_binary"],
        "workers": 8, "placement": placement(cpus, numa_node),
        "cpus": list(cpus), "numa_memory_node": numa_node,
        "filesystem": subprocess.check_output(["findmnt", "-n", "-o", "SOURCE,FSTYPE,OPTIONS", "-T", str(root)], text=True).strip(),
        "max_records_per_trial": MAX_RECORDS,
        "max_records_per_worker": MAX_RECORDS_PER_WORKER, "max_db_file_bytes": DB_FILE_LIMIT,
        "duration_ms": DURATION_MS, "time_limit_seconds": TRIAL_TIMEOUT_SECONDS,
        "durability_note": "Immediate and None are distinct redb API modes; close/reopen is not a power-failure test",
        "boundary_note": "Critical section = begin + inserts + commit (incl. commit I/O) of one fixed-shape request; reads are outside every write lock",
        "db_retention": "DB bytes+SHA256 and raw stdout/stderr retained per cell; DB files removed after hashing to bound disk",
        "seeds": SEEDS,
    }
    write_json(root / "manifest.json", identity)
    return identity


def load_identity(root):
    identity = json.loads((root / "manifest.json").read_text())
    if identity.get("schema") != 2:
        raise RuntimeError("manifest predates the patched-redb runner; prepare a fresh output root")
    build = load_build(Path(identity["build_dir"]))
    for key in ("redb_source", "harness_sources_sha256"):
        if build[key] != identity[key]:
            raise RuntimeError(f"{key} changed after preparation; create fresh output root")
    if identity["runner_sha256"] != digest(Path(__file__).resolve()):
        raise RuntimeError("runner changed after preparation; create fresh output root")
    for name, entry in identity["binaries"].items():
        if digest(root / entry["file"]) != entry["sha256"]:
            raise RuntimeError(f"prepared binary changed: {name}")
    return identity


def trial(root, identity, cohort, durability, variant, repeat, seed, smoke=False):
    name = f"r{repeat}_{cohort}_{durability}_{variant}"
    directory = root / ("smoke" if smoke else "runs") / name
    binary_name = identity["variant_binary"][variant]
    binary = root / identity["binaries"][binary_name]["file"]
    command = [str(binary), "--database", str(directory / "db.redb"),
               "--variant", variant, "--cohort", cohort, "--durability", durability,
               "--seed", str(seed), "--duration-ms", str(DURATION_MS),
               "--cpus", ",".join(map(str, identity["cpus"]))]
    if identity["numa_memory_node"] is not None:
        command = ["numactl", f"--membind={identity['numa_memory_node']}", *command]
    entry = command_capture(command, directory, timeout=TRIAL_TIMEOUT_SECONDS, limit_db=True)
    entry.update({"cohort": cohort, "durability": durability, "variant": variant,
                  "repeat": repeat, "seed": seed, "smoke": smoke, "binary": binary_name,
                  "binary_sha256": identity["binaries"][binary_name]["sha256"]})
    db_file = directory / "db.redb"
    if db_file.exists():
        entry["database_bytes"] = db_file.stat().st_size
        entry["database_sha256"] = digest(db_file)
    if entry["exit_code"] == 0 and not entry["timed_out"]:
        try:
            result = json.loads((directory / "stdout").read_text())
            if (result["variant"], result["cohort"], result["durability"], result["seed"],
                result["smoke_transactions"], len(result["workers"])) != (
                    variant, cohort, durability, seed, None, 8):
                raise ValueError("result identity mismatch")
            if (result["max_records"] != MAX_RECORDS
                or result["max_records_per_worker"] != MAX_RECORDS_PER_WORKER
                or result["verified_live_records"] > MAX_RECORDS
                or any(w["completed_records"] > MAX_RECORDS_PER_WORKER for w in result["workers"])):
                raise ValueError("record capacity guard mismatch")
            if entry.get("database_bytes", 0) > DB_FILE_LIMIT:
                raise ValueError("database file capacity guard mismatch")
            if not result["reopened_exact"] and durability == "immediate":
                raise ValueError("Immediate close/reopen mismatch")
            entry["results"] = result
        except (ValueError, KeyError) as error:
            entry["parse_error"] = str(error)
    write_json(directory / "result.json", entry)
    if db_file.exists():
        db_file.unlink()  # All outcomes retain raw stdout/stderr, status and database hash.
    return "results" in entry


def cells(smoke):
    cohorts = SMOKE_COHORTS if smoke else COHORTS
    return [(r, cohort, durability) for r in range(len(SEEDS))
            for cohort in cohorts for durability in DURABILITIES]


def run_matrix(root, smoke=False):
    identity = load_identity(root)
    check_affinity(identity["cpus"])
    if (root / ("smoke" if smoke else "runs")).exists():
        raise RuntimeError("refusing to overwrite prior raw trials; use a fresh output root")
    errors = []
    for repeat, cohort, durability in cells(smoke):
        seed = SEEDS[repeat]
        order = list(VARIANTS)
        random.Random(seed ^ (COHORTS.index(cohort) << 8) ^ (DURABILITIES.index(durability) << 16)).shuffle(order)
        for variant in order:
            if not trial(root, identity, cohort, durability, variant, repeat, seed, smoke):
                errors.append(f"r{repeat} {cohort} {durability} {variant}")
    if errors:
        raise RuntimeError(f"{len(errors)} failed cells; raw failure evidence retained: {errors}")


def jain(values):
    square = sum(x*x for x in values)
    return sum(values)**2 / (len(values) * square) if square else None


def quantile(hist, proportion):
    count = sum(hist)
    if not count:
        return None
    threshold = max(1, int(count * proportion + 0.999999))
    total = 0
    for bucket, n in enumerate(hist):
        total += n
        if total >= threshold:
            return (1 << (bucket + 1)) / 1_000_000  # inclusive bin upper bound, milliseconds
    raise AssertionError("invalid latency histogram")


def metrics(entry):
    result = entry["results"]
    workers = result["workers"]
    seconds = result["duration_ms"] / 1000
    records = [sum(w["window_records"]) for w in workers]
    tx = [sum(w["window_transactions"]) for w in workers]
    small = [i for i, w in enumerate(workers) if w["records_per_transaction"] == 1]
    large = [i for i in range(8) if i not in small]
    windows = [[w["window_transactions"][t] for w in workers] for t in range(8)]
    histogram = [sum(w["response_ns_log2"][b] for w in workers) for b in range(64)]
    short_histogram = [sum(workers[i]["response_ns_log2"][b] for i in small) for b in range(64)]
    long_histogram = [sum(workers[i]["response_ns_log2"][b] for i in large) for b in range(64)]
    return {"throughput_tx_s": sum(tx)/seconds, "throughput_records_s": sum(records)/seconds,
            "short_tx_s": sum(tx[i] for i in small)/seconds,
            "long_tx_s": sum(tx[i] for i in large)/seconds,
            "tx_jain": jain(tx), "max_zero_progress_windows": max(
                sum(row[i] == 0 for row in windows) for i in range(8)),
            "worker_tx": tx, "worker_records": records, "windows_tx": windows,
            "response_p50_ms_upper": quantile(histogram, 0.50),
            "response_p99_ms_upper": quantile(histogram, 0.99),
            "short_response_p99_ms_upper": quantile(short_histogram, 0.99),
            "long_response_p99_ms_upper": quantile(long_histogram, 0.99),
            "process_cpu_seconds": result["process_cpu_ns"] / 1e9,
            "reopened_exact": result["reopened_exact"],
            "reopen_error": result["reopen_error"]}


COMPARISONS = (("refactored", "native"), ("bridge_mutex", "refactored"),
               ("fc_pq", "fc"), ("fc_pq", "native"), ("mcs", "bridge_mutex"), ("fc", "bridge_mutex"))


def paired_effects(rows, cohorts):
    lookup = {(r["repeat"], r["cohort"], r["durability"], r["variant"]): r
              for r in rows if not r["failure"]}
    effects = []
    for repeat in range(len(SEEDS)):
        for cohort in cohorts:
            for durability in DURABILITIES:
                for treatment, base_name in COMPARISONS:
                    base = lookup.get((repeat, cohort, durability, base_name))
                    other = lookup.get((repeat, cohort, durability, treatment))
                    if base is None or other is None:
                        continue
                    effects.append({
                        "repeat": repeat, "cohort": cohort, "durability": durability,
                        "comparison": f"{treatment}_vs_{base_name}",
                        "records_per_s_ratio": other["throughput_records_s"]/base["throughput_records_s"]
                            if base["throughput_records_s"] else None,
                        "transactions_per_s_ratio": other["throughput_tx_s"]/base["throughput_tx_s"]
                            if base["throughput_tx_s"] else None,
                        "process_cpu_s_delta": other["process_cpu_seconds"]-base["process_cpu_seconds"],
                        "tx_jain_delta": (other["tx_jain"]-base["tx_jain"]
                                          if other["tx_jain"] is not None and base["tx_jain"] is not None else None),
                    })
    return effects


def control_check(rows, cohorts):
    """refactored vs native per cohort/durability: repeat-level spread is the noise."""
    checks = []
    for cohort in cohorts:
        for durability in DURABILITIES:
            values = {}
            for variant in ("native", "refactored"):
                values[variant] = [r["throughput_tx_s"] for r in rows if not r["failure"]
                                   and r["cohort"] == cohort and r["durability"] == durability
                                   and r["variant"] == variant]
            native, refactored = values["native"], values["refactored"]
            if len(native) < 2 or len(refactored) < 2:
                checks.append({"cohort": cohort, "durability": durability, "verdict": "insufficient repeats"})
                continue
            native_median, refactored_median = statistics.median(native), statistics.median(refactored)
            spread = max((max(v) - min(v)) / statistics.median(v) for v in (native, refactored))
            ratio = refactored_median / native_median
            checks.append({"cohort": cohort, "durability": durability,
                           "native_tx_s": native, "refactored_tx_s": refactored,
                           "median_ratio_refactored_over_native": ratio,
                           "noise_relative_range": spread,
                           "verdict": "within noise" if abs(ratio - 1) <= spread else "outside noise"})
    return checks


def analyze(root, smoke=False):
    kind = "smoke" if smoke else "runs"
    cohorts = SMOKE_COHORTS if smoke else COHORTS
    paths = sorted((root / kind).glob("*/result.json"))
    expected = len(cells(smoke)) * len(VARIANTS)
    if len(paths) != expected:
        raise RuntimeError(f"expected {expected} {kind} raw results, found {len(paths)}")
    rows = []
    for path in paths:
        entry = json.loads(path.read_text())
        row = {key: entry[key] for key in ("cohort", "durability", "variant", "repeat", "seed", "binary", "binary_sha256")}
        row["raw_result"] = str(path.relative_to(root))
        row["failure"] = entry.get("parse_error") or ("timeout" if entry["timed_out"] else
                          f"exit {entry['exit_code']}" if entry["exit_code"] != 0 else None)
        if "results" in entry:
            row.update(metrics(entry))
        rows.append(row)
    analysis = root / ("analysis-smoke" if smoke else "analysis")
    analysis.mkdir(exist_ok=False)
    write_json(analysis / "summary.json", {
        "cohort_kind": kind, "rows": rows, "paired_effects": paired_effects(rows, cohorts),
        "refactored_vs_native": control_check(rows, cohorts),
        "failures": sum(bool(r["failure"]) for r in rows),
        "caveats": ["Each durability mode is a distinct regime; None is not a durable-commit claim.",
                    "The critical section includes commit I/O; Immediate throughput is dominated by fsync.",
                    "Reads are outside every write lock and are not measured here.",
                    "Transaction size changes completed mix; total tx/s is not isolated lock overhead.",
                    "Smoke uses 3 repetitions: the control check is a noise-bounded sanity check, not a precise estimate.",
                    "Close/reopen does not simulate power failure."]})
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    fig, axes = plt.subplots(len(DURABILITIES), len(cohorts), figsize=(4.7 * len(cohorts), 8), sharex=True, squeeze=False)
    for column, cohort in enumerate(cohorts):
        for row_index, durability in enumerate(DURABILITIES):
            axis = axes[row_index][column]
            for index, variant in enumerate(VARIANTS):
                values = [r["throughput_tx_s"] for r in rows if not r["failure"] and r["cohort"] == cohort
                          and r["durability"] == durability and r["variant"] == variant]
                axis.plot([index] * len(values), values, "o", alpha=.65)
            axis.set_title(f"{cohort} / {durability}")
            axis.set_xticks(range(len(VARIANTS)), VARIANTS, rotation=40)
            axis.set_ylabel(f"verified tx/s ({DURATION_MS // 1000}s window)")
    fig.tight_layout()
    fig.savefig(analysis / "throughput.png", dpi=160)
    plt.close(fig)
    return len([r for r in rows if r["failure"]])


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--prepare-only", action="store_true")
    mode.add_argument("--smoke", action="store_true")
    mode.add_argument("--run", action="store_true", help="formal matrix; only when explicitly intended")
    mode.add_argument("--analyze-only", action="store_true", help="analyze the formal matrix")
    mode.add_argument("--analyze-smoke", action="store_true", help="analyze the smoke cohort")
    parser.add_argument("--output-root", type=Path, required=True)
    parser.add_argument("--build-dir", type=Path, default=OUT, help="verified build.py output (prepare)")
    parser.add_argument("--cpus", type=parse_cpus, help="eight distinct CPUs; default at prepare: 8-15")
    parser.add_argument("--numa-node", type=parse_numa_node, default=argparse.SUPPRESS,
                        help="memory node for numactl; default at prepare: 0; none disables binding")
    args = parser.parse_args()
    root = args.output_root.resolve()
    if args.prepare_only:
        cpus = args.cpus if args.cpus is not None else DEFAULT_CPUS
        numa_node = getattr(args, "numa_node", 0)
        prepare(root, args.build_dir.resolve(), cpus, numa_node)
    elif args.smoke or args.run:
        identity = load_identity(root)
        if args.cpus is not None and list(args.cpus) != identity["cpus"]:
            raise RuntimeError("--cpus differs from prepared campaign")
        if hasattr(args, "numa_node") and args.numa_node != identity["numa_memory_node"]:
            raise RuntimeError("--numa-node differs from prepared campaign")
        run_matrix(root, smoke=args.smoke)
    elif analyze(root, smoke=args.analyze_smoke):
        raise RuntimeError("analysis contains failed cells; inspect raw results")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"redb campaign: {error}", file=sys.stderr)
        sys.exit(1)
