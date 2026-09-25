#!/usr/bin/env python3
"""Bounded native redb write-transaction matrix; one immutable trial directory per cell."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import resource
import shutil
import subprocess
import sys

HERE = Path(__file__).resolve().parents[1]
MANIFEST = HERE / "integration/redb/Cargo.toml"
BACKENDS = ("native", "mutex", "mcs", "fc", "fc_pq")
COHORTS = ("all1", "half1_half8", "half1_half64")
DURABILITIES = ("immediate", "none")
DEFAULT_CPUS = tuple(range(8, 16))
SEEDS = (0x7BEF523C1678F92D, 0xDB3102F89158C44B, 0x7E3BB4A250D1E66F)
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


def source_hashes():
    sources = [MANIFEST, MANIFEST.with_name("Cargo.lock"),
               HERE / "integration/redb/src/main.rs", HERE / "integration/redb_transactions.py"]
    sources += sorted((HERE / "crates/libdlock").rglob("*.rs"))
    sources += sorted((HERE / "crates/libdlock").rglob("*.c"))
    sources += sorted((HERE / "crates/libdlock").rglob("*.h"))
    sources += sorted((HERE / "c").rglob("*.c"))
    sources += sorted((HERE / "c").rglob("*.h"))
    sources += [HERE / "crates/libdlock/Cargo.toml", HERE / "crates/libdlock/build.rs"]
    return {str(p.relative_to(HERE)): digest(p) for p in sources}


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


def positive_int(value):
    try:
        number = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("expected a positive integer") from error
    if number < 1:
        raise argparse.ArgumentTypeError("expected a positive integer")
    return number


def check_affinity(cpus):
    if os.sched_getaffinity(0) != set(cpus):
        raise RuntimeError(f"run under taskset -c {','.join(map(str, cpus))} (exactly eight requested CPUs)")

def command_capture(command, directory, timeout=None, env=None, limit_db=False):
    directory.mkdir(parents=True, exist_ok=False)
    (directory / "command.json").write_text(json.dumps(command) + "\n")
    def limits():
        if limit_db:
            resource.setrlimit(resource.RLIMIT_FSIZE, (DB_FILE_LIMIT, DB_FILE_LIMIT))
    try:
        completed = subprocess.run(command, cwd=HERE, env=env, timeout=timeout,
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


def prepare(root, target, cpus, numa_node, build_jobs):
    check_affinity(cpus)
    root.mkdir(parents=True, exist_ok=False)
    target.mkdir(parents=True, exist_ok=True)
    binaries = root / "binaries"
    binaries.mkdir()
    build_info = {}
    for kind in ("primary", "profile"):
        command = ["cargo", "build", "--manifest-path", str(MANIFEST),
                   "--release", "--locked", "--bin", "redb_transactions",
                   "--target-dir", str(target / kind), "-j", str(build_jobs)]
        if kind == "profile":
            command += ["--features", "redb_profile"]
        log = root / "build" / kind
        entry = command_capture(command, log, timeout=1200)
        write_json(log / "result.json", entry)
        if entry["exit_code"] != 0 or entry["timed_out"]:
            raise RuntimeError(f"{kind} build failed, evidence: {log}")
        built = target / kind / "release/redb_transactions"
        snapshot = binaries / kind
        shutil.copy2(built, snapshot)
        snapshot.chmod(0o555)
        build_info[kind] = {"sha256": digest(snapshot), "file": str(snapshot.relative_to(root)),
                            "command": command}
    source = source_hashes()
    identity = {
        "git_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=HERE, text=True).strip(),
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "redb_version": "=3.1.0", "source_sha256": source, "binaries": build_info,
        "workers": 8, "cpus": list(cpus), "numa_memory_node": numa_node,
        "filesystem": subprocess.check_output(["findmnt", "-n", "-o", "SOURCE,FSTYPE,OPTIONS", "-T", str(root)], text=True).strip(),
        "max_records_per_trial": MAX_RECORDS,
        "max_records_per_worker": MAX_RECORDS_PER_WORKER, "max_db_file_bytes": DB_FILE_LIMIT,
        "time_limit_seconds": TRIAL_TIMEOUT_SECONDS, "durability_note":
            "Immediate and None are distinct redb API modes; close/reopen is not a power-failure test",
        "service_note": "profile service begins after begin_write, includes commit I/O (not CPU); only completed-in-window requests credited, no boundary clipping; FC-PQ internal billing may also include begin_write/admin",
        "db_retention": "DB bytes+SHA256 and raw stdout/stderr retained per cell; DB files removed after hashing to bound disk",
        "seeds": SEEDS,
    }
    write_json(root / "manifest.json", identity)
    return identity


def load_identity(root):
    identity = json.loads((root / "manifest.json").read_text())
    if identity["source_sha256"] != source_hashes():
        raise RuntimeError("source changed after preparation; create fresh output root")
    for kind in ("primary", "profile"):
        file = root / identity["binaries"][kind]["file"]
        if digest(file) != identity["binaries"][kind]["sha256"]:
            raise RuntimeError(f"prepared binary changed: {file}")
    return identity


def trial(root, identity, kind, cohort, durability, backend, repeat, seed, smoke=False):
    name = f"r{repeat}_{cohort}_{durability}_{kind}_{backend}"
    directory = root / ("smoke" if smoke else "runs") / name
    binary = root / "binaries" / kind
    command = [str(binary), "--database", str(directory / "db.redb"),
               "--backend", backend, "--cohort", cohort, "--durability", durability,
               "--seed", str(seed), "--duration-ms", "2000",
               "--cpus", ",".join(map(str, identity["cpus"]))]
    if identity["numa_memory_node"] is not None:
        command = ["numactl", f"--membind={identity['numa_memory_node']}", *command]
    if smoke:
        command += ["--smoke-transactions", "4"]
    entry = command_capture(command, directory, timeout=TRIAL_TIMEOUT_SECONDS, limit_db=True)
    entry.update({"cohort": cohort, "durability": durability, "backend": backend,
                  "repeat": repeat, "seed": seed, "build": kind, "smoke": smoke})
    db_file = directory / "db.redb"
    if db_file.exists():
        entry["database_bytes"] = db_file.stat().st_size
        entry["database_sha256"] = digest(db_file)
    if entry["exit_code"] == 0 and not entry["timed_out"]:
        try:
            result = json.loads((directory / "stdout").read_text())
            if (result["backend"], result["cohort"], result["durability"], result["seed"],
                result["profile"], len(result["workers"])) != (
                    backend, cohort, durability, seed, kind == "profile", 8):
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


def run_matrix(root, smoke=False):
    identity = load_identity(root)
    check_affinity(identity["cpus"])
    if (root / ("smoke" if smoke else "runs")).exists():
        raise RuntimeError("refusing to overwrite prior raw trials; use a fresh output root")
    errors = []
    if smoke:
        combinations = ((0, COHORTS[-1], durability) for durability in DURABILITIES)
    else:
        combinations = ((r, cohort, durability) for r in range(len(SEEDS))
                        for cohort in COHORTS for durability in DURABILITIES)
    for repeat, cohort, durability in combinations:
        seed = SEEDS[repeat]
        order = list(BACKENDS)
        random.Random(seed ^ (COHORTS.index(cohort) << 8) ^ (DURABILITIES.index(durability) << 16)).shuffle(order)
        for kind in ("primary", "profile"):
            for backend in order:
                if not trial(root, identity, kind, cohort, durability, backend, repeat, seed, smoke):
                    errors.append(f"r{repeat} {cohort} {durability} {kind} {backend}")
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
    output = {"throughput_tx_s": sum(tx)/seconds, "throughput_records_s": sum(records)/seconds,
              "short_tx_s": sum(tx[i] for i in small)/seconds,
              "long_tx_s": sum(tx[i] for i in large)/seconds,
              "short_records_s": sum(records[i] for i in small)/seconds,
              "long_records_s": sum(records[i] for i in large)/seconds,
              "tx_jain": jain(tx), "max_zero_progress_windows": max(
                  sum(row[i] == 0 for row in windows) for i in range(8)),
              "worker_tx": tx, "worker_records": records, "windows_tx": windows,
              "response_p50_ms_upper": quantile(histogram, 0.50),
              "response_p95_ms_upper": quantile(histogram, 0.95),
              "response_p99_ms_upper": quantile(histogram, 0.99),
              "short_response_p99_ms_upper": quantile(short_histogram, 0.99),
              "long_response_p99_ms_upper": quantile(long_histogram, 0.99),
              "process_cpu_seconds": result["process_cpu_ns"] / 1e9,
              "reopened_exact": result["reopened_exact"],
              "reopen_error": result["reopen_error"]}
    if result["profile"]:
        service = [w["service_wall_ns"] for w in workers]
        output["service_wall_jain"] = jain(service)
        output["worker_service_wall_seconds"] = [x/1e9 for x in service]
        output["service_wall_p99_ms_upper"] = quantile(
            [sum(w["service_ns_log2"][b] for w in workers) for b in range(64)], 0.99)
    return output

def paired_effects(rows):
    lookup = {(r["repeat"], r["cohort"], r["durability"], r["build"], r["backend"]): r
              for r in rows if not r["failure"]}
    effects = []
    for repeat in range(len(SEEDS)):
        for cohort in COHORTS:
            for durability in DURABILITIES:
                for build in ("primary", "profile"):
                    for comparator in ("fc", "native"):
                        base = lookup.get((repeat, cohort, durability, build, comparator))
                        pq = lookup.get((repeat, cohort, durability, build, "fc_pq"))
                        if base is None or pq is None:
                            continue
                        effects.append({
                            "repeat": repeat, "cohort": cohort, "durability": durability,
                            "build": build, "comparison": f"fc_pq_vs_{comparator}",
                            "records_per_s_ratio": pq["throughput_records_s"]/base["throughput_records_s"]
                                if base["throughput_records_s"] else None,
                            "transactions_per_s_ratio": pq["throughput_tx_s"]/base["throughput_tx_s"]
                                if base["throughput_tx_s"] else None,
                            "short_tx_per_s_ratio": pq["short_tx_s"]/base["short_tx_s"]
                                if base["short_tx_s"] else None,
                            "long_tx_per_s_ratio": pq["long_tx_s"]/base["long_tx_s"]
                                if base["long_tx_s"] else None,
                            "process_cpu_s_delta": pq["process_cpu_seconds"]-base["process_cpu_seconds"],
                            "response_p99_ms_upper_delta": (
                                pq["response_p99_ms_upper"]-base["response_p99_ms_upper"]
                                if pq["response_p99_ms_upper"] is not None
                                and base["response_p99_ms_upper"] is not None else None),
                            "service_wall_jain_delta": (
                                pq["service_wall_jain"]-base["service_wall_jain"]
                                if build == "profile" and comparator == "fc"
                                and pq["service_wall_jain"] is not None
                                and base["service_wall_jain"] is not None else None),
                            "zero_progress_windows_delta": (
                                pq["max_zero_progress_windows"]-base["max_zero_progress_windows"]),
                        })
    perturbation = []
    for repeat in range(len(SEEDS)):
        for cohort in COHORTS:
            for durability in DURABILITIES:
                for backend in BACKENDS:
                    primary = lookup.get((repeat, cohort, durability, "primary", backend))
                    profile = lookup.get((repeat, cohort, durability, "profile", backend))
                    if primary is not None and profile is not None:
                        perturbation.append({
                            "repeat": repeat, "cohort": cohort, "durability": durability,
                            "backend": backend,
                            "profile_vs_primary_records_per_s_ratio":
                                profile["throughput_records_s"]/primary["throughput_records_s"]
                                if primary["throughput_records_s"] else None,
                            "profile_vs_primary_transactions_per_s_ratio":
                                profile["throughput_tx_s"]/primary["throughput_tx_s"]
                                if primary["throughput_tx_s"] else None,
                        })
    return effects, perturbation



def analyze(root):
    paths = sorted((root / "runs").glob("*/result.json"))
    expected = len(SEEDS) * len(COHORTS) * len(DURABILITIES) * 2 * len(BACKENDS)
    if len(paths) != expected:
        raise RuntimeError(f"expected {expected} formal raw results, found {len(paths)}")
    entries = [json.loads(path.read_text()) for path in paths]
    rows = []
    for path, entry in zip(paths, entries):
        row = {key: entry[key] for key in ("cohort", "durability", "backend", "repeat", "seed", "build")}
        row["raw_result"] = str(path.relative_to(root))
        row["failure"] = entry.get("parse_error") or ("timeout" if entry["timed_out"] else
                          f"exit {entry['exit_code']}" if entry["exit_code"] != 0 else None)
        if "results" in entry:
            row.update(metrics(entry))
        rows.append(row)
    analysis = root / "analysis"
    analysis.mkdir(exist_ok=False)
    effects, perturbation = paired_effects(rows)
    write_json(analysis / "summary.json", {
        "rows": rows, "paired_effects": effects, "profile_perturbation": perturbation,
        "failures": sum(bool(r["failure"]) for r in rows),
        "caveats": ["Each mode is a distinct durability regime; None is not a durable-commit claim.",
                    "Profile service starts after begin_write, ends after commit, includes I/O and is not CPU; native writer-lock waiting is requester latency. FC-PQ billing may include begin_write/admin. Service allocation credits only requests completed within the 2s window, never clips requests crossing its boundary.",
                    "Native redb is the baseline; wrappers do not improve the native engine itself.",
                    "Transaction size changes completed mix; total tx/s is not isolated scheduler overhead.",
                    "Windowed progress and response tails complement whole-run Jain; zero windows cannot rule out shorter stalls.",
                    "Close/reopen does not simulate power failure. Profile build timestamps the callback and may perturb scheduling."]})
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    fig, axes = plt.subplots(2, 3, figsize=(14, 8), sharex=True)
    for column, cohort in enumerate(COHORTS):
        for row_index, durability in enumerate(DURABILITIES):
            axis = axes[row_index][column]
            for build, style in (("primary", "o"), ("profile", "x")):
                series = [next((r for r in rows if r["repeat"] == repeat and r["cohort"] == cohort
                    and r["durability"] == durability and r["build"] == build and r["backend"] == backend), None)
                    for backend in BACKENDS for repeat in range(len(SEEDS))]
                for backend_index, backend in enumerate(BACKENDS):
                    values = [r["throughput_records_s"] for r in series[backend_index*len(SEEDS):(backend_index+1)*len(SEEDS)]
                              if r is not None and not r["failure"]]
                    if values:
                        axis.plot([backend_index]*len(values), values, style, alpha=.65,
                                  label=build if backend_index == 0 else None)
            axis.set_title(f"{cohort} / {durability}")
            axis.set_xticks(range(len(BACKENDS)), BACKENDS, rotation=40)
            axis.set_ylabel("verified records/s (2s window)")
            if column == 0 and row_index == 0:
                axis.legend()
    fig.tight_layout()
    fig.savefig(analysis / "throughput.png", dpi=160)
    plt.close(fig)
    fig, axes = plt.subplots(1, 2, figsize=(13, 5))
    for kind, axis in (("primary", axes[0]), ("profile", axes[1])):
        for cohort in COHORTS[1:]:
            for durability in DURABILITIES:
                subset = [r for r in rows if r["build"] == kind and r["cohort"] == cohort
                    and r["durability"] == durability and r["backend"] in ("fc", "fc_pq") and not r["failure"]]
                xkey = "tx_jain" if kind == "primary" else "service_wall_jain"
                for backend, marker in (("fc", "o"), ("fc_pq", "x")):
                    points = [r for r in subset if r["backend"] == backend and r[xkey] is not None]
                    if points:
                        axis.scatter([r[xkey] for r in points],
                                     [r["throughput_records_s"] for r in points],
                                     marker=marker, label=f"{cohort} {durability} {backend}", alpha=.75)
        axis.set_title(f"{kind}: FC/FC-PQ paired tradeoff")
        axis.set_xlabel("requester " + ("transaction-count" if kind == "primary" else "executor service-wall") + " Jain")
        axis.set_ylabel("verified records/s")
        axis.legend(fontsize=7)
    fig.tight_layout()
    fig.savefig(analysis / "fairness_tradeoff.png", dpi=160)
    plt.close(fig)
    return len([r for r in rows if r["failure"]])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--prepare-only", action="store_true")
    mode.add_argument("--smoke", action="store_true")
    mode.add_argument("--run", action="store_true")
    mode.add_argument("--analyze-only", action="store_true")
    parser.add_argument("--output-root", type=Path, required=True)
    parser.add_argument("--target-root", type=Path, default=HERE / ".worktree/redb-cargo-target")
    parser.add_argument("--cpus", type=parse_cpus, help="eight distinct CPUs; default at prepare: 8-15")
    parser.add_argument("--numa-node", type=parse_numa_node, default=argparse.SUPPRESS,
                        help="memory node for numactl; default at prepare: 0; none disables binding")
    parser.add_argument("--build-jobs", type=positive_int, default=8)
    args = parser.parse_args()
    root = args.output_root.resolve()
    if args.prepare_only:
        cpus = args.cpus if args.cpus is not None else DEFAULT_CPUS
        numa_node = getattr(args, "numa_node", 0)
        prepare(root, args.target_root.resolve(), cpus, numa_node, args.build_jobs)
    elif args.smoke or args.run:
        identity = load_identity(root)
        if args.cpus is not None and list(args.cpus) != identity["cpus"]:
            raise RuntimeError("--cpus differs from prepared campaign")
        if hasattr(args, "numa_node") and args.numa_node != identity["numa_memory_node"]:
            raise RuntimeError("--numa-node differs from prepared campaign")
        run_matrix(root, smoke=args.smoke)
    elif analyze(root):
        raise RuntimeError("analysis contains failed cells; inspect raw results")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"redb campaign: {error}", file=sys.stderr)
        sys.exit(1)
