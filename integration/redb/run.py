#!/usr/bin/env python3
"""Fresh-process redb write-path trials; one immutable trial directory per cell.

--prepare-only snapshots binaries from a verified, instrumented build (build.py)
and freezes their source/patch/binary identity and CPU/NUMA placement. --smoke
runs a small timed cohort (all variants, the client sweep, 3 repetitions,
including the transfer workload) whose analysis includes the upstream_gate-vs-upstream
control check. --run runs the formal matrix only when explicitly requested.
Every failure is retained.

Clients: a cell with c clients runs c pinned requesters on the first c prepared
CPUs, and the trial process is restricted to exactly those CPUs (never more
client threads than CPUs). Waiting: MCS/FC/FC-PQ spin, upstream/upstream_gate/
std_mutex block (Mutex/Condvar, futex), U-SCL may yield, futex-wait or sleep. Cohorts: all1 = every client writes 1 record per request
(any c); half1_halfK = half the requests carry 1 record and half K: with c even
the first c/2 clients write 1 record and the other c/2 write K (2 clients = one
of each); a single client alternates 1, K, 1, K, ... (the same request mix
without contention; per-client fairness is trivially 1). transfer (smoke only;
any c) = every client submits one transfer closure per request (2 reads + 2
updates of a fixed account table, aborted on insufficient balance); the cell
fails unless the total is conserved live and after close/reopen.

Durability: None is the primary regime; Immediate (fsync inside the critical
section) is a control, reported second.

--perf runs the separate cache-counter profile cohort (None; all1 and
half1_half64; every client count; 3 repetitions) under `perf stat` for the whole
process, counting only the client phase (the harness enables/disables perf via
its control FIFOs). Timed cells (--smoke, --run) never run perf.
Every cell (timed and perf) samples each client CPU's clock (sysfs
cpuinfo_avg_freq) and busy time (/proc/stat); see FREQ_*.
--analyze-perf normalises the counters per committed transaction and record and
reports the effective clock (cycles / ref-cycles x TSC rate), throughput scaled
to the reference clock, and cells whose clients ran at mixed clocks.

Power setup (fixed at --prepare-only, checked before every matrix and after
every cell): S0 = stock (schedutil, scaling range = cpuinfo range, turbo on);
S1 = fixed clock F (--fixed-ghz, default 3.0): performance governor and
scaling_min_freq = scaling_max_freq = F on every cpufreq policy, turbo on, C6
enabled on the run CPUs; S2 = S1 with C6 disabled on the run CPUs. The runner
needs no root; it refuses a host whose observed state differs from the setup.
Under S1/S2 a cell whose clock is not within CLOCK_TOLERANCE of F is flagged
(clock_off_target), and the reference clock of normalised throughput is F
(S0: the TSC rate). --sustain-probe spins every run CPU for a few seconds and
reports each CPU's effective clock, to choose the highest F the host holds.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import re
import resource
import shutil
import statistics
import subprocess
import sys
import threading
import time

from integration.redb.build import OUT, load_build

HERE = Path(__file__).resolve().parents[2]
VARIANTS = ('upstream', 'upstream_gate', 'std_mutex', 'mcs', 'uscl', 'fc', 'fc_pq')
# Legacy names: results recorded before the 2026-09-29 rename use these; the analysis loaders map them.
LEGACY_NAMES = {'native': 'upstream', 'refactored': 'upstream_gate', 'bridge_mutex': 'std_mutex'}
COHORTS = ('all1', 'half1_half8', 'half1_half64')
# Transfer: 2 reads + 2 updates per closure, abort on insufficient balance. Smoke only.
TRANSFER_COHORT = 'transfer'
SMOKE_COHORTS = ('all1', 'half1_half64', TRANSFER_COHORT)
ORDER_COHORTS = COHORTS + (TRANSFER_COHORT,)  # stable cohort index: variant-order seed, row order
CLIENTS = (1, 2, 4, 8)
DURABILITIES = ('none', 'immediate')  # primary regime first; Immediate is the control
DEFAULT_CPUS = tuple(range(8, 16))
SEEDS = (0x7BEF523C1678F92D, 0xDB3102F89158C44B, 0x7E3BB4A250D1E66F)
DURATION_MS = 2000
TRIAL_TIMEOUT_SECONDS = 30
DB_FILE_LIMIT = 512 * 1024 * 1024
PERF_COHORTS = ('all1', 'half1_half64')
PERF_DURABILITIES = ('none',)
# User-mode only (perf_event_paranoid 2). Checked 2026-09-28 with perf 7.2.5 on
# this Sapphire Rapids host: 8 general-purpose counters per logical CPU;
# instructions runs on fixed counter 0; cycles:u takes a general-purpose counter
# (8 GP events plus cycles:u multiplex, plus instructions:u do not; fixed counter
# 1 is presumably held by the NMI watchdog). ref_tsc is the fixed-counter-2
# encoding 0x0300 (TSC rate). perf 7.2.5 resolves plain "ref-cycles" to the
# programmable 0x013c instead, so it is not used. parse_perf rejects any event
# below 100 % running.
PERF_EVENTS = {
    "instructions": "instructions:u",
    "cycles": "cycles:u",
    "ref_cycles": "cpu_clk_unhalted.ref_tsc:u",              # unhalted user cycles at the TSC rate
    "hitm_loads": "mem_load_l3_hit_retired.xsnp_fwd:u",     # loads served by HitM from another on-socket core
    "hitm_supplied": "core_snoop_response.i_fwd_m:u",       # this core's modified lines taken (invalidated) by a snoop
    "l2_miss_loads": "mem_load_retired.l2_miss:u",
    "l2_miss_all": "l2_rqsts.miss:u",                       # demand + RFO + prefetch
    "llc_miss": "longest_lat_cache.miss:u",
}
# Per-client clock: cpuinfo_avg_freq is the kernel's APERF/MPERF over the last
# 4 ms tick (kHz); an idle CPU reports a stale or requested value, so a clock
# sample counts only if /proc/stat shows its CPU at least FREQ_BUSY_MIN busy in
# the enclosing busy interval (every FREQ_BUSY_EVERY-th sample; /proc/stat costs
# ~0.5 ms, a clock read of 8 CPUs ~0.12 ms). Clocks change within a process, so
# a client's clock is the mean of its accepted samples (dense sampling matched
# perf's cycles/ref_tsc within 1-2 %). A cell is "mixed" when the clocks of the
# clients with at least FREQ_MIN_SAMPLES accepted samples differ by more than
# FREQ_MIXED_RATIO (base 2.2 GHz vs turbo >= 3.0 GHz is >= 1.36). Rule fixed
# before the rerun.
FREQ_SAMPLE_KINDS = ("smoke", "runs", "perf")
FREQ_SAMPLE_SECONDS = 0.05
FREQ_BUSY_EVERY = 4
FREQ_BUSY_MIN = 0.5
FREQ_MIN_SAMPLES = 8
FREQ_MIXED_RATIO = 1.25
POWER_SETUPS = ("S0", "S1", "S2")
DEFAULT_FIXED_GHZ = 3.0
CLOCK_TOLERANCE = 0.02  # S1/S2: a cell's clock must be within +-2 % of F
CPU_SYS = Path("/sys/devices/system/cpu")
MAX_CLIENTS = 8  # the trial binary's worker limit
MAX_RECORDS_PER_WORKER = 8_000_000
MAX_RECORDS = MAX_CLIENTS * MAX_RECORDS_PER_WORKER


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
    if not cpus or len(set(cpus)) != len(cpus) or any(cpu < 0 for cpu in cpus):
        raise argparse.ArgumentTypeError("distinct nonnegative CPUs are required")
    return cpus


def cohort_runs(cohort, clients):
    """Whether a cohort is defined at a client count (see module docstring)."""
    return cohort in ("all1", TRANSFER_COHORT) or clients == 1 or clients % 2 == 0


def sweep_clients(cpus):
    """Client counts of the sweep that fit the prepared CPU set."""
    return [c for c in CLIENTS if c <= min(len(cpus), MAX_CLIENTS)]


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
        raise RuntimeError(f"run under taskset -c {','.join(map(str, cpus))} (exactly the requested CPUs)")


def command_capture(command, directory, timeout=None, limit_db=False, env=None):
    directory.mkdir(parents=True, exist_ok=False)
    (directory / "command.json").write_text(json.dumps(command) + "\n")
    def limits():
        if limit_db:
            resource.setrlimit(resource.RLIMIT_FSIZE, (DB_FILE_LIMIT, DB_FILE_LIMIT))
    try:
        completed = subprocess.run(command, cwd=HERE, timeout=timeout,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   preexec_fn=limits if limit_db else None, check=False,
                                   env=None if env is None else {**os.environ, **env})
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


def prepare(root, build_dir, cpus, numa_node, power_setup, fixed_ghz, variants):
    check_affinity(cpus)
    power = power_preflight(cpus, power_setup, fixed_ghz)
    build = load_build(build_dir)
    if not build["instrumented"]:
        raise RuntimeError("uninstrumented builds are for the overhead check only")
    if "fcpq_fast_path" not in build["binaries"]["patched"]["resolved_features"].get("libdlock", []):
        raise RuntimeError("FC-PQ must be built with fcpq_fast_path")
    clients = sweep_clients(cpus)
    if not clients:
        raise RuntimeError("no client count of the sweep fits the CPU set")
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
                           "features": entry["features"], "resolved_features": entry["resolved_features"],
                           "build_command": entry["command"]}
    identity = {
        "schema": 4,
        "build_dir": str(build_dir),
        "redb_source": build["redb_source"],
        "harness_sources_sha256": build["harness_sources_sha256"],
        "runner_sha256": digest(Path(__file__).resolve()),
        "build_git_head": build["git_head"], "build_git_dirty_paths": build["git_dirty_paths"],
        "git_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=HERE, text=True).strip(),
        "rustc": build["rustc"], "binaries": snapshots, "variant_binary": build["variant_binary"],
        "clients": clients, "placement": placement(cpus, numa_node),
        "variants": list(variants),
        "perf": perf_identity(), "tsc": tsc_identity(), "power": power,
        "frequency_sampling": {"kinds": list(FREQ_SAMPLE_KINDS), "interval_s": FREQ_SAMPLE_SECONDS,
                               "busy_every": FREQ_BUSY_EVERY, "busy_min": FREQ_BUSY_MIN,
                               "min_samples": FREQ_MIN_SAMPLES, "mixed_ratio": FREQ_MIXED_RATIO},
        "cpus": list(cpus), "numa_memory_node": numa_node,
        "client_cpus_note": "c clients use the first c CPUs; the trial process is restricted to them",
        "filesystem": subprocess.check_output(["findmnt", "-n", "-o", "SOURCE,FSTYPE,OPTIONS", "-T", str(root)], text=True).strip(),
        "max_records_per_trial": MAX_RECORDS,
        "max_records_per_worker": MAX_RECORDS_PER_WORKER, "max_db_file_bytes": DB_FILE_LIMIT,
        "duration_ms": DURATION_MS, "time_limit_seconds": TRIAL_TIMEOUT_SECONDS,
        "durability_note": "None is the primary regime, Immediate a control; distinct redb API modes; close/reopen is not a power-failure test",
        "service_note": "service = rdtscp ticks from begin returning to commit/abort returning, on the executing thread, charged to the requester",
        "boundary_note": "Critical section = one whole write transaction submitted as a closure (begin + closure + commit/abort, incl. commit I/O); reads are outside every write lock",
        "db_retention": "DB bytes+SHA256 and raw stdout/stderr retained per cell; DB files removed after hashing to bound disk",
        "seeds": SEEDS,
    }
    write_json(root / "manifest.json", identity)
    return identity


def perf_identity():
    path = shutil.which("perf")
    if path is None:
        return None
    return {"path": path, "version": subprocess.check_output([path, "--version"], text=True).strip(),
            "events": PERF_EVENTS, "tsc": tsc_identity(),
            "perf_event_paranoid": Path("/proc/sys/kernel/perf_event_paranoid").read_text().strip()}


def tsc_identity():
    """Nominal TSC rate from CPUID leaf 0x15 (crystal Hz x EBX / EAX), if the cpuid tool exists.

    ref_tsc counts at this rate; the analysis uses each cell's measured
    tsc_ticks_per_ns and reports the CPUID value next to it."""
    path = shutil.which("cpuid")
    if path is None:
        return None
    raw = subprocess.check_output([path, "-1", "-r", "-l", "0x15"], text=True).strip().splitlines()[-1].strip()
    regs = {name: int(value, 16) for name, value in re.findall(r"(e[a-d]x)=0x([0-9a-f]+)", raw)}
    eax, ebx, ecx = regs.get("eax", 0), regs.get("ebx", 0), regs.get("ecx", 0)
    return {"cpuid_0x15": raw, "crystal_hz": ecx, "ratio": [ebx, eax],
            "hz": ecx * ebx / eax if eax and ebx and ecx else None}


def sysfs(path):
    try:
        return path.read_text().strip()
    except OSError:
        return None


def power_state(cpus):
    """Observed host power state: intel_pstate, every cpufreq policy, and per run CPU
    its cpufreq settings and cpuidle states (name, disable flag, exit latency)."""
    pstate = {key: sysfs(CPU_SYS / "intel_pstate" / key)
              for key in ("status", "no_turbo", "min_perf_pct", "max_perf_pct")}
    policies = {}
    for policy in sorted((CPU_SYS / "cpufreq").glob("policy*"), key=lambda p: int(p.name[6:])):
        policies[policy.name] = {key: sysfs(policy / key) for key in (
            "scaling_governor", "scaling_min_freq", "scaling_max_freq", "cpuinfo_min_freq", "cpuinfo_max_freq")}
    per_cpu = {}
    for cpu in cpus:
        base = CPU_SYS / f"cpu{cpu}"
        idle = []
        for state in sorted((base / "cpuidle").glob("state*"), key=lambda p: int(p.name[5:])):
            idle.append({"state": state.name, "name": sysfs(state / "name"), "disable": sysfs(state / "disable"),
                         "latency_us": sysfs(state / "latency")})
        per_cpu[str(cpu)] = {**{key: sysfs(base / "cpufreq" / key) for key in (
            "scaling_driver", "scaling_governor", "scaling_min_freq", "scaling_max_freq", "scaling_cur_freq")},
            "cpuidle": idle}
    return {"intel_pstate": pstate, "policies": policies, "cpus": per_cpu}


def check_power(state, setup, fixed_ghz):
    """Mismatches between an observed power_state and the setup (see module docstring)."""
    problems = []
    if state["intel_pstate"]["no_turbo"] != "0":
        problems.append(f"intel_pstate/no_turbo = {state['intel_pstate']['no_turbo']}, expected 0 (turbo on)")
    if not state["policies"]:
        problems.append("no cpufreq policies visible")
    target = str(round(fixed_ghz * 1e6)) if setup in ("S1", "S2") else None
    for name, policy in state["policies"].items():
        if setup == "S0":
            expected = ("schedutil", policy["cpuinfo_min_freq"], policy["cpuinfo_max_freq"])
        else:
            expected = ("performance", target, target)
        observed = (policy["scaling_governor"], policy["scaling_min_freq"], policy["scaling_max_freq"])
        if observed != expected:
            problems.append(f"{name}: governor/min/max = {'/'.join(map(str, observed))}, "
                            f"expected {'/'.join(map(str, expected))}")
    want_c6_disabled = "1" if setup == "S2" else "0"
    for cpu, entry in state["cpus"].items():
        c6 = [s for s in entry["cpuidle"] if s["name"] == "C6"]
        if not c6:
            problems.append(f"cpu{cpu}: no cpuidle state named C6")
        elif c6[0]["disable"] != want_c6_disabled:
            problems.append(f"cpu{cpu}: C6 disable = {c6[0]['disable']}, expected {want_c6_disabled}")
    return problems


def power_preflight(cpus, setup, fixed_ghz):
    """Refuse unless the host is in the requested setup; returns the manifest's power record."""
    state = power_state(cpus)
    problems = check_power(state, setup, fixed_ghz)
    if problems:
        shown = "; ".join(problems[:6]) + (f"; ... ({len(problems)} mismatches)" if len(problems) > 6 else "")
        raise RuntimeError(f"host is not in power setup {setup}: {shown}")
    return {"setup": setup, "fixed_ghz": fixed_ghz if setup in ("S1", "S2") else None,
            "clock_tolerance": CLOCK_TOLERANCE, "state": state}


def parse_perf(path):
    """perf stat -x, output -> ({name: count}, {name: {"running_ns", "percent"}}).

    Raises on a missing, unsupported or uncounted event. The caller rejects
    multiplexed events (< 100 % running) after retaining the fractions."""
    by_event = {event: name for name, event in PERF_EVENTS.items()}
    counts, running = {}, {}
    for line in path.read_text().splitlines():
        if not line or line.startswith("#"):
            continue
        fields = line.split(",")
        value, event, percent = fields[0], fields[2], fields[4]
        if event not in by_event:
            continue
        if value in ("<not counted>", "<not supported>"):
            raise ValueError(f"perf event {event}: {value}")
        counts[by_event[event]] = int(float(value))
        running[by_event[event]] = {"running_ns": int(fields[3]), "percent": float(percent)}
    if set(counts) != set(PERF_EVENTS):
        raise ValueError(f"perf output lacks events: {sorted(set(PERF_EVENTS) - set(counts))}")
    return counts, running


def cpu_clocks(cpus):
    """cpuinfo_avg_freq per CPU in kHz (None if unreadable)."""
    khz = []
    for cpu in cpus:
        try:
            khz.append(int(Path(f"/sys/devices/system/cpu/cpu{cpu}/cpufreq/cpuinfo_avg_freq").read_text()))
        except (OSError, ValueError):
            khz.append(None)
    return khz


def cpu_jiffies(cpus):
    """[busy, total] /proc/stat jiffies per CPU (None if absent)."""
    jiffies = {}
    with open("/proc/stat") as stream:
        for line in stream:
            if line.startswith("cpu") and line[3].isdigit():
                name, *values = line.split()
                values = [int(v) for v in values[:8]]  # user nice system idle iowait irq softirq steal
                jiffies[int(name[3:])] = [sum(values) - values[3] - values[4], sum(values)]
    return [jiffies.get(cpu) for cpu in cpus]


class FrequencySampler:
    """Samples the client CPUs' clocks every FREQ_SAMPLE_SECONDS and their busy
    jiffies every FREQ_BUSY_EVERY-th sample while a trial runs (see FREQ_*)."""

    def __init__(self, cpus):
        self.cpus, self.samples, self.done = list(cpus), [], threading.Event()
        self.thread = threading.Thread(target=self._run, daemon=True)

    def _run(self):
        begin, index = time.monotonic(), 0
        while True:
            sample = {"t": round(time.monotonic() - begin, 4), "khz": cpu_clocks(self.cpus)}
            if index % FREQ_BUSY_EVERY == 0:
                sample["jiffies"] = cpu_jiffies(self.cpus)
            self.samples.append(sample)
            index += 1
            if self.done.wait(FREQ_SAMPLE_SECONDS):
                return

    def start(self):
        self.thread.start()

    def stop(self):
        self.done.set()
        self.thread.join()
        return {"cpus": self.cpus, "interval_s": FREQ_SAMPLE_SECONDS, "busy_every": FREQ_BUSY_EVERY,
                "samples": self.samples}


def load_identity(root):
    identity = json.loads((root / "manifest.json").read_text())
    if identity.get("schema") != 4:
        raise RuntimeError("manifest predates the power-setup runner; prepare a fresh output root")
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


def trial(root, identity, cohort, clients, durability, variant, repeat, seed, kind):
    name = f"r{repeat}_{cohort}_c{clients}_{durability}_{variant}"
    directory = root / kind / name
    smoke = kind == "smoke"
    binary_name = identity["variant_binary"][variant]
    binary = root / identity["binaries"][binary_name]["file"]
    cpus = identity["cpus"][:clients]
    cpu_list = ",".join(map(str, cpus))
    command = [str(binary), "--database", str(directory / "db.redb"),
               "--variant", variant, "--cohort", cohort, "--durability", durability,
               "--seed", str(seed), "--duration-ms", str(DURATION_MS), "--cpus", cpu_list]
    if identity["numa_memory_node"] is not None:
        command = ["numactl", f"--membind={identity['numa_memory_node']}", *command]
    env = None
    if kind == "perf":
        # Control FIFOs live outside the (not yet created) immutable cell directory.
        control = root / "perf-control" / name
        control.mkdir(parents=True)
        ctl, ack = control / "ctl", control / "ack"
        os.mkfifo(ctl)
        os.mkfifo(ack)
        command = [identity["perf"]["path"], "stat", "-x,", "-o", str(directory / "perf.csv"),
                   "--delay=-1", "--control", f"fifo:{ctl},{ack}",
                   "-e", ",".join(PERF_EVENTS.values()), "--", *command]
        env = {"REDB_PERF_CONTROL": f"{ctl},{ack}"}
    command = ["taskset", "-c", cpu_list, *command]
    sampler = FrequencySampler(cpus) if kind in FREQ_SAMPLE_KINDS else None
    if sampler is not None:
        sampler.start()
    entry = command_capture(command, directory, timeout=TRIAL_TIMEOUT_SECONDS, limit_db=True, env=env)
    if sampler is not None:
        entry["frequency_samples"] = sampler.stop()
    power = identity["power"]
    entry["power_problems"] = check_power(power_state(identity["cpus"]), power["setup"], power["fixed_ghz"])
    entry.update({"cohort": cohort, "clients": clients, "durability": durability, "variant": variant,
                  "repeat": repeat, "seed": seed, "smoke": smoke, "binary": binary_name,
                  "binary_sha256": identity["binaries"][binary_name]["sha256"]})
    db_file = directory / "db.redb"
    if db_file.exists():
        entry["database_bytes"] = db_file.stat().st_size
        entry["database_sha256"] = digest(db_file)
    if entry["exit_code"] == 0 and not entry["timed_out"]:
        try:
            if entry["power_problems"]:
                raise ValueError(f"power setup {power['setup']} changed during the cell: {entry['power_problems'][:3]}")
            result = json.loads((directory / "stdout").read_text())
            if (result["variant"], result["cohort"], result["durability"], result["seed"],
                result["smoke_transactions"], result["clients"], result["cpus"], len(result["workers"])) != (
                    variant, cohort, durability, seed, None, clients, cpus, clients):
                raise ValueError("result identity mismatch")
            if not result["build"]["service_time"] or (
                    variant == "fc_pq" and not result["build"]["fcpq_fast_path"]):
                raise ValueError("binary lacks service time or the FC-PQ fast path")
            if (result["max_records"] != MAX_RECORDS
                or result["max_records_per_worker"] != MAX_RECORDS_PER_WORKER
                or result["verified_live_records"] > MAX_RECORDS
                or any(w["completed_records"] > MAX_RECORDS_PER_WORKER for w in result["workers"])):
                raise ValueError("record capacity guard mismatch")
            if entry.get("database_bytes", 0) > DB_FILE_LIMIT:
                raise ValueError("database file capacity guard mismatch")
            if not result["reopened_exact"] and durability == "immediate":
                raise ValueError("Immediate close/reopen mismatch")
            transfer = result["transfer"]
            if (transfer is not None) != (cohort == TRANSFER_COHORT):
                raise ValueError("transfer check present for the wrong cohort")
            if transfer is not None and not (transfer["conserved_live"] and transfer["reopened_identical"]):
                raise ValueError("transfer total not conserved live and after reopen")
            if result["perf_counted"] != (kind == "perf"):
                raise ValueError("perf control state does not match the cohort")
            if kind == "perf":
                entry["perf_counts"], entry["perf_running"] = parse_perf(directory / "perf.csv")
                multiplexed = {n: r["percent"] for n, r in entry["perf_running"].items() if r["percent"] < 99.99}
                if multiplexed:
                    raise ValueError(f"perf events multiplexed (% running): {multiplexed}")
            entry["results"] = result
        except (ValueError, KeyError, OSError, IndexError) as error:
            entry["parse_error"] = str(error)
    write_json(directory / "result.json", entry)
    if db_file.exists():
        db_file.unlink()  # All outcomes retain raw stdout/stderr, status and database hash.
    return "results" in entry


KIND_COHORTS = {"smoke": SMOKE_COHORTS, "runs": COHORTS, "perf": PERF_COHORTS}
KIND_DURABILITIES = {"smoke": DURABILITIES, "runs": DURABILITIES, "perf": PERF_DURABILITIES}


def groups(kind, clients):
    """(cohort, clients) pairs of the smoke cohort, formal matrix or perf cohort, in report order."""
    return [(cohort, c) for cohort in KIND_COHORTS[kind] for c in clients if cohort_runs(cohort, c)]


def cells(kind, clients):
    return [(r, cohort, c, durability) for r in range(len(SEEDS))
            for cohort, c in groups(kind, clients) for durability in KIND_DURABILITIES[kind]]


def run_matrix(root, kind):
    identity = load_identity(root)
    check_affinity(identity["cpus"])
    power_preflight(identity["cpus"], identity["power"]["setup"], identity["power"]["fixed_ghz"])
    if kind == "perf" and not identity.get("perf"):
        raise RuntimeError("perf was not available at preparation")
    if (root / kind).exists():
        raise RuntimeError("refusing to overwrite prior raw trials; use a fresh output root")
    errors = []
    for repeat, cohort, clients, durability in cells(kind, identity["clients"]):
        seed = SEEDS[repeat]
        order = [v for v in VARIANTS if v in identity["variants"]]
        random.Random(seed ^ (ORDER_COHORTS.index(cohort) << 8) ^ (DURABILITIES.index(durability) << 16)
                      ^ (clients << 24)).shuffle(order)
        for variant in order:
            if not trial(root, identity, cohort, clients, durability, variant, repeat, seed, kind):
                errors.append(f"r{repeat} {cohort} c{clients} {durability} {variant}")
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


def median_range(values):
    values = [v for v in values if v is not None]
    if not values:
        return None
    return {"median": statistics.median(values), "min": min(values), "max": max(values), "n": len(values)}


def metrics(entry):
    result = entry["results"]
    workers = result["workers"]
    n = len(workers)
    seconds = result["duration_ms"] / 1000
    records = [sum(w["window_records"]) for w in workers]
    tx = [sum(w["window_transactions"]) for w in workers]
    service = [w["service_tsc_ticks"] for w in workers]
    total_service = sum(service)
    # Pure 1-record clients vs pure K-record clients; a lone mixed client is neither.
    # Transfer: every client alike, no short/long split.
    transfer = entry["cohort"] == TRANSFER_COHORT
    small = [] if transfer else [i for i, w in enumerate(workers) if w["request_sizes"] == [1]]
    large = [] if transfer else [i for i, w in enumerate(workers)
                                 if len(w["request_sizes"]) == 1 and w["request_sizes"] != [1]]
    split = len(small) + len(large) == n
    windows = [[w["window_transactions"][t] for w in workers] for t in range(len(workers[0]["window_transactions"]))]
    histogram = [sum(w["response_ns_log2"][b] for w in workers) for b in range(64)]
    short_histogram = [sum(workers[i]["response_ns_log2"][b] for i in small) for b in range(64)]
    long_histogram = [sum(workers[i]["response_ns_log2"][b] for i in large) for b in range(64)]
    hits = [w["fast_path_hits"] for w in workers]
    calls = sum(w["completed_transactions"] for w in workers)
    aborted = sum(w["aborted_transactions"] for w in workers)
    share = [x / total_service for x in service] if total_service else None
    return {"throughput_tx_s": sum(tx)/seconds, "throughput_records_s": sum(records)/seconds,
            "short_tx_s": sum(tx[i] for i in small)/seconds if split else None,
            "long_tx_s": sum(tx[i] for i in large)/seconds if split else None,
            "tx_jain": jain(tx), "service_jain": jain(service),
            "worker_service_share": share,
            "long_service_share": sum(share[i] for i in large) if share and large else None,
            # Bodies are serialised, so this is the busy fraction of the write path (<= 1).
            "service_utilization": total_service / (result["tsc_ticks_per_ns"] * result["duration_ms"] * 1e6),
            "fast_path_hit_rate": (sum(hits) / calls if calls and all(h is not None for h in hits) else None),
            "max_zero_progress_windows": max(sum(row[i] == 0 for row in windows) for i in range(n)),
            "worker_tx": tx, "worker_records": records, "worker_service_tsc": service, "windows_tx": windows,
            "aborted_transactions": aborted,
            "aborted_fraction": aborted / (aborted + calls) if aborted + calls else None,
            "response_p50_ms_upper": quantile(histogram, 0.50),
            "response_p99_ms_upper": quantile(histogram, 0.99),
            "short_response_p99_ms_upper": quantile(short_histogram, 0.99) if split else None,
            "long_response_p99_ms_upper": quantile(long_histogram, 0.99) if split else None,
            "process_cpu_seconds": result["process_cpu_ns"] / 1e9, "tsc_ghz": result["tsc_ticks_per_ns"],
            "reopened_exact": result["reopened_exact"],
            "reopen_error": result["reopen_error"]}


COMPARISONS = (("upstream_gate", "upstream"), ("std_mutex", "upstream_gate"),
               ("fc_pq", "fc"), ("fc_pq", "upstream"), ("fc_pq", "mcs"), ("fc_pq", "uscl"),
               ("mcs", "std_mutex"), ("uscl", "mcs"), ("fc", "std_mutex"))


def ratio(a, b):
    return a / b if b else None


def delta(a, b):
    return a - b if a is not None and b is not None else None


def paired_effects(rows, group_list):
    lookup = {(r["repeat"], r["cohort"], r["clients"], r["durability"], r["variant"]): r
              for r in rows if not r["failure"]}
    effects = []
    for durability in DURABILITIES:
        for cohort, clients in group_list:
            for repeat in range(len(SEEDS)):
                for treatment, base_name in COMPARISONS:
                    base = lookup.get((repeat, cohort, clients, durability, base_name))
                    other = lookup.get((repeat, cohort, clients, durability, treatment))
                    if base is None or other is None:
                        continue
                    effects.append({
                        "repeat": repeat, "cohort": cohort, "clients": clients, "durability": durability,
                        "comparison": f"{treatment}_vs_{base_name}",
                        "records_per_s_ratio": ratio(other["throughput_records_s"], base["throughput_records_s"]),
                        "transactions_per_s_ratio": ratio(other["throughput_tx_s"], base["throughput_tx_s"]),
                        "process_cpu_s_delta": other["process_cpu_seconds"]-base["process_cpu_seconds"],
                        "tx_jain_delta": delta(other["tx_jain"], base["tx_jain"]),
                        "service_jain_delta": delta(other["service_jain"], base["service_jain"]),
                    })
    return effects


def control_check(rows, group_list):
    """upstream_gate vs upstream per durability/cohort/clients: repeat-level spread is the noise."""
    checks = []
    for durability in DURABILITIES:
        for cohort, clients in group_list:
            values = {}
            for variant in ("upstream", "upstream_gate"):
                values[variant] = [r["throughput_tx_s"] for r in rows if not r["failure"]
                                   and r["cohort"] == cohort and r["clients"] == clients
                                   and r["durability"] == durability and r["variant"] == variant]
            upstream, upstream_gate = values["upstream"], values["upstream_gate"]
            base = {"durability": durability, "cohort": cohort, "clients": clients}
            if len(upstream) < 2 or len(upstream_gate) < 2:
                checks.append({**base, "verdict": "insufficient repeats"})
                continue
            upstream_median, upstream_gate_median = statistics.median(upstream), statistics.median(upstream_gate)
            spread = max((max(v) - min(v)) / statistics.median(v) for v in (upstream, upstream_gate))
            value = upstream_gate_median / upstream_median
            checks.append({**base, "upstream_tx_s": upstream, "upstream_gate_tx_s": upstream_gate,
                           "median_ratio_upstream_gate_over_upstream": value,
                           "noise_relative_range": spread,
                           "verdict": "within noise" if abs(value - 1) <= spread else "outside noise"})
    return checks


TABLE_METRICS = ("throughput_tx_s", "throughput_records_s", "service_jain", "tx_jain",
                 "long_service_share", "process_cpu_seconds", "service_utilization", "fast_path_hit_rate",
                 "clock_ghz", "client_clock_spread", "tx_s_at_ref", "records_s_at_ref")


def clock_counts(chosen):
    """Cells, flagged mixed, off target (S1/S2) and clock-undecidable counts of a table row."""
    return {"cells": len(chosen),
            "mixed_clock_cells": sum(r.get("mixed_clock") is True for r in chosen),
            "clock_off_target_cells": sum(r.get("clock_off_target") is True for r in chosen),
            "clock_unknown_cells": sum(r.get("clock_ghz") is None for r in chosen)}


def cell_table(rows, group_list):
    """Median [min, max] per durability x cohort x clients x variant, None durability first."""
    present = {r["variant"] for r in rows}
    table = []
    for durability in DURABILITIES:
        for cohort, clients in group_list:
            for variant in (v for v in VARIANTS if v in present):
                chosen = [r for r in rows if not r["failure"] and (r["durability"], r["cohort"], r["clients"],
                                                                     r["variant"]) == (durability, cohort, clients, variant)]
                table.append({"durability": durability, "cohort": cohort, "clients": clients, "variant": variant,
                              **{m: median_range([r.get(m) for r in chosen]) for m in TABLE_METRICS},
                              **clock_counts(chosen)})
    return table


def markdown(table, checks):
    def fmt(cell, digits, scale=1.0):
        if cell is None:
            return "—"
        return f"{cell['median']*scale:.{digits}f} [{cell['min']*scale:.{digits}f}, {cell['max']*scale:.{digits}f}]"
    lines = ["| durability | cohort | clients | variant | tx/s | records/s | service_jain | tx_jain | "
             "long service share | CPU-s | fast-path hit rate | clock GHz | off target | tx/s @ref |",
             "|" + "---|" * 14]
    for row in table:
        lines.append(f"| {row['durability']} | {row['cohort']} | {row['clients']} | {row['variant']} | "
                     f"{fmt(row['throughput_tx_s'], 0)} | {fmt(row['throughput_records_s'], 0)} | "
                     f"{fmt(row['service_jain'], 3)} | {fmt(row['tx_jain'], 3)} | "
                     f"{fmt(row['long_service_share'], 3)} | {fmt(row['process_cpu_seconds'], 2)} | "
                     f"{fmt(row['fast_path_hit_rate'], 3)} | {fmt(row['clock_ghz'], 2)} | "
                     f"{row['clock_off_target_cells']}/{row['cells']} | {fmt(row['tx_s_at_ref'], 0)} |")
    lines += ["", "| durability | cohort | clients | upstream_gate/upstream (median tx/s) | noise range | verdict |",
              "|---|---|---|---|---|---|"]
    for check in checks:
        if "median_ratio_upstream_gate_over_upstream" in check:
            lines.append(f"| {check['durability']} | {check['cohort']} | {check['clients']} | "
                         f"{check['median_ratio_upstream_gate_over_upstream']:.3f} | "
                         f"{check['noise_relative_range']:.3f} | {check['verdict']} |")
        else:
            lines.append(f"| {check['durability']} | {check['cohort']} | {check['clients']} | — | — | {check['verdict']} |")
    return "\n".join(lines) + "\n"


def perf_metrics(entry):
    """Counters over the client phase per committed transaction and per record.

    Counting runs from just before the clients start to after all are joined, so
    it covers every committed request (the window plus the requests draining at
    its end): normalise by completed_transactions/records, not the window counts.
    """
    counts = entry["perf_counts"]
    workers = entry["results"]["workers"]
    tx = sum(w["completed_transactions"] for w in workers)
    records = sum(w["completed_records"] for w in workers)
    out = {"perf_committed_transactions": tx, "perf_committed_records": records, "perf_counts": counts,
           "ipc": counts["instructions"] / counts["cycles"] if counts.get("cycles") else None}
    for name, value in counts.items():
        out[f"{name}_per_tx"] = value / tx if tx else None
        out[f"{name}_per_record"] = value / records if records else None
    if "perf_running" in entry:
        out["perf_min_running_pct"] = min(r["percent"] for r in entry["perf_running"].values())
    if counts.get("ref_cycles"):
        # ref_tsc ticks at the TSC rate while a thread is unhalted in user mode, so
        # cycles / ref_cycles is the ref-cycle-weighted mean clock over all threads.
        result = entry["results"]
        tsc_ghz = result["tsc_ticks_per_ns"]
        out.update({"effective_ghz": counts["cycles"] / counts["ref_cycles"] * tsc_ghz,
                    "ref_busy_cpus": counts["ref_cycles"] / (tsc_ghz * result["elapsed_ns"])})
    return out


def client_clocks(entry):
    """Per-client mean clock from the sampler and the mixed-clock flag (rule: see FREQ_*)."""
    sampling = entry["frequency_samples"]
    samples, cpus = sampling["samples"], sampling["cpus"]
    marks = [i for i, s in enumerate(samples) if "jiffies" in s]
    per_client, busy_samples = [], []
    for cpu in range(len(cpus)):
        accepted = []
        for a, b in zip(marks, marks[1:]):
            before, after = samples[a]["jiffies"][cpu], samples[b]["jiffies"][cpu]
            if before is None or after is None or after[1] <= before[1]:
                continue
            if (after[0] - before[0]) / (after[1] - before[1]) >= FREQ_BUSY_MIN:
                accepted += [s["khz"][cpu] / 1e6 for s in samples[a + 1:b + 1] if s["khz"][cpu] is not None]
        busy_samples.append(len(accepted))
        per_client.append(statistics.mean(accepted) if len(accepted) >= FREQ_MIN_SAMPLES else None)
    valid = [f for f in per_client if f is not None]
    spread = max(valid) / min(valid) if len(valid) >= 2 else None
    return {"client_ghz": per_client, "client_busy_samples": busy_samples,
            "client_clock_ghz": statistics.mean(valid) if valid else None,
            "client_clock_spread": spread,
            # None: fewer than two clients with enough busy samples, so undecidable.
            "mixed_clock": None if spread is None else spread > FREQ_MIXED_RATIO}


def clock_metrics(row, identity):
    """Cell clock, reference clock, normalised throughput and the S1/S2 off-target flag.

    The clock is perf's effective_ghz when counted (process-wide, not the
    combiner's), else the sampler's client mean (a heuristic; whole child
    process). The reference is F under S1/S2 and the TSC rate under S0 (and for
    roots prepared before power setups existed)."""
    power = identity.get("power") or {"setup": "S0", "fixed_ghz": None}
    target = power.get("fixed_ghz")
    if row.get("effective_ghz"):
        clock, basis = row["effective_ghz"], "perf"
    elif row.get("client_clock_ghz"):
        clock, basis = row["client_clock_ghz"], "sampler"
    else:
        clock, basis = None, None
    reference = target or row.get("tsc_ghz")
    out = {"power_setup": power["setup"], "clock_target_ghz": target, "reference_ghz": reference,
           "clock_ghz": clock, "clock_basis": basis,
           "clock_off_target": None if target is None or clock is None else abs(clock / target - 1) > CLOCK_TOLERANCE,
           "clients_off_target": None if target is None or "client_ghz" not in row else
           sum(abs(f / target - 1) > CLOCK_TOLERANCE for f in row["client_ghz"] if f is not None)}
    if clock and reference:
        out["tx_s_at_ref"] = row["throughput_tx_s"] * reference / clock
        out["records_s_at_ref"] = row["throughput_records_s"] * reference / clock
    return out


def clock_summary(rows, identity):
    """Power setup and the fraction of cells flagged off target (S1/S2)."""
    ok = [r for r in rows if not r["failure"]]
    decided = [r for r in ok if r.get("clock_off_target") is not None]
    flagged = sum(r["clock_off_target"] for r in decided)
    power = identity.get("power") or {"setup": "S0", "fixed_ghz": None}
    return {"power_setup": power["setup"], "fixed_ghz": power.get("fixed_ghz"), "clock_tolerance": CLOCK_TOLERANCE,
            "cells": len(ok), "clock_decided_cells": len(decided), "clock_off_target_cells": flagged,
            "clock_off_target_fraction": flagged / len(decided) if decided else None,
            "clock_unknown_cells": len(ok) - len(decided) if power.get("fixed_ghz") else None}


PERF_TABLE = ("throughput_tx_s", "throughput_records_s", "service_jain", "hitm_loads_per_tx", "hitm_supplied_per_tx",
              "l2_miss_loads_per_tx", "l2_miss_all_per_tx", "llc_miss_per_tx", "instructions_per_tx",
              "cycles_per_tx", "ipc", "hitm_loads_per_record", "llc_miss_per_record", "process_cpu_seconds",
              "ref_cycles_per_tx", "effective_ghz", "ref_busy_cpus", "tx_s_at_ref", "records_s_at_ref",
              "client_clock_ghz", "client_clock_spread", "perf_min_running_pct")


def analyze_perf(root):
    identity, rows = load_rows(root, "perf")
    group_list = groups("perf", identity["clients"])
    present = {r["variant"] for r in rows}
    table = []
    for cohort, clients in group_list:
        for variant in (v for v in VARIANTS if v in present):
            chosen = [r for r in rows if not r["failure"] and (r["cohort"], r["clients"], r["variant"]) ==
                      (cohort, clients, variant)]
            # Excludes cells flagged mixed or off target; undecidable cells stay in.
            kept = [r for r in chosen if r.get("mixed_clock") is not True and r.get("clock_off_target") is not True]
            table.append({"durability": "none", "cohort": cohort, "clients": clients, "variant": variant,
                          **{m: median_range([r.get(m) for r in chosen]) for m in PERF_TABLE},
                          **clock_counts(chosen),
                          "tx_s_at_ref_not_flagged": median_range([r.get("tx_s_at_ref") for r in kept]),
                          "records_s_at_ref_not_flagged": median_range([r.get("records_s_at_ref") for r in kept])})
    tsc = [r["tsc_ghz"] for r in rows if r.get("tsc_ghz")]
    clock = clock_summary(rows, identity)
    analysis = root / "analysis-perf"
    analysis.mkdir(exist_ok=False)
    write_json(analysis / "summary.json", {
        "cohort_kind": "perf", "perf": identity.get("perf"), "groups": group_list, "table": table, "rows": rows,
        "failures": sum(bool(r["failure"]) for r in rows),
        "measured_tsc_ghz": median_range(tsc), "clock": clock,
        "min_running_pct": min((r["perf_min_running_pct"] for r in rows if "perf_min_running_pct" in r), default=None),
        "caveats": ["Profile cohort: every cell runs under perf stat; its throughput is not a primary result.",
                    "Process totals over all threads (perf stat cannot attribute a command's threads separately); user mode only.",
                    "Counting covers the client phase only (enabled after all clients are ready, disabled after all are joined), including requests draining after the 2 s window; normalised by all committed transactions/records.",
                    "Spinning waiters' cycles and instructions are included, so cycles/tx of spinning locks grows with clients regardless of the body.",
                    "hitm_loads = loads served by a HitM snoop from another core on this socket; hitm_supplied = this core's modified lines taken by snoops.",
                    "effective_ghz = cycles / ref_tsc x measured TSC rate: the mean clock of all threads weighted by their unhalted user time, not the clock of the thread running the body.",
                    "tx_s_at_ref = window tx/s x reference / effective_ghz, reference = F under S1/S2, the TSC rate under S0; assumes the serial path runs at the process mean clock.",
                    f"clock_off_target (S1/S2): effective_ghz outside F +- {CLOCK_TOLERANCE:.0%}; flagged cells are kept in the raw rows and excluded from *_not_flagged.",
                    "client clocks (heuristic): mean per-CPU cpuinfo_avg_freq sampled every 50 ms by the runner over the whole child process, only samples inside a 200 ms interval with the CPU >= 50 % busy (/proc/stat); mixed_clock = client means differ by > 1.25x (>= 2 clients with >= 8 samples, else undecidable).",
                    "*_not_flagged excludes cells flagged mixed or off target; undecidable cells stay in."]})
    def fmt(cell, digits):
        if cell is None:
            return "—"
        return f"{cell['median']:.{digits}f} [{cell['min']:.{digits}f}, {cell['max']:.{digits}f}]"
    lines = [f"Power setup {clock['power_setup']}, fixed clock {clock['fixed_ghz']} GHz; off target: "
             f"{clock['clock_off_target_cells']}/{clock['clock_decided_cells']} decided cells.", "",
             "| cohort | clients | variant | tx/s | HITM loads/tx | HITM supplied/tx | L2-miss loads/tx | "
             "L2 misses/tx | LLC misses/tx | instr/tx | cycles/tx | IPC |", "|" + "---|" * 12]
    for row in table:
        lines.append(f"| {row['cohort']} | {row['clients']} | {row['variant']} | {fmt(row['throughput_tx_s'], 0)} | "
                     f"{fmt(row['hitm_loads_per_tx'], 0)} | {fmt(row['hitm_supplied_per_tx'], 0)} | "
                     f"{fmt(row['l2_miss_loads_per_tx'], 0)} | {fmt(row['l2_miss_all_per_tx'], 0)} | "
                     f"{fmt(row['llc_miss_per_tx'], 1)} | {fmt(row['instructions_per_tx'], 0)} | "
                     f"{fmt(row['cycles_per_tx'], 0)} | {fmt(row['ipc'], 2)} |")
    lines += ["", "| cohort | clients | variant | tx/s | effective GHz | client GHz | client spread | mixed | "
              "off target | user CPUs (ref) | cycles/tx | tx/s @ref | tx/s @ref (not flagged) | "
              "records/s @ref (not flagged) |", "|" + "---|" * 14]
    for row in table:
        lines.append(f"| {row['cohort']} | {row['clients']} | {row['variant']} | {fmt(row['throughput_tx_s'], 0)} | "
                     f"{fmt(row['effective_ghz'], 2)} | {fmt(row['client_clock_ghz'], 2)} | "
                     f"{fmt(row['client_clock_spread'], 2)} | {row['mixed_clock_cells']}/{row['cells']} | "
                     f"{row['clock_off_target_cells']}/{row['cells']} | "
                     f"{fmt(row['ref_busy_cpus'], 2)} | {fmt(row['cycles_per_tx'], 0)} | {fmt(row['tx_s_at_ref'], 0)} | "
                     f"{fmt(row['tx_s_at_ref_not_flagged'], 0)} | {fmt(row['records_s_at_ref_not_flagged'], 0)} |")
    (analysis / "summary.md").write_text("\n".join(lines) + "\n")
    return sum(bool(r["failure"]) for r in rows)


def load_rows(root, kind):
    identity = json.loads((root / "manifest.json").read_text())
    paths = sorted((root / kind).glob("*/result.json"))
    expected = len(cells(kind, identity["clients"])) * len(identity.get("variants", VARIANTS))
    if len(paths) != expected:
        raise RuntimeError(f"expected {expected} {kind} raw results, found {len(paths)}")
    rows = []
    for path in paths:
        entry = json.loads(path.read_text())
        row = {key: entry[key] for key in ("cohort", "clients", "durability", "variant", "repeat", "seed",
                                           "binary", "binary_sha256")}
        row["variant"] = LEGACY_NAMES.get(row["variant"], row["variant"])
        row["binary"] = LEGACY_NAMES.get(row["binary"], row["binary"])
        row["raw_result"] = str(path.relative_to(root))
        row["failure"] = entry.get("parse_error") or ("timeout" if entry["timed_out"] else
                          f"exit {entry['exit_code']}" if entry["exit_code"] != 0 else None)
        if "results" in entry:
            row.update(metrics(entry))
        if "perf_counts" in entry and "results" in entry:
            row.update(perf_metrics(entry))
        if "frequency_samples" in entry and "results" in entry:
            row.update(client_clocks(entry))
        if "results" in entry:
            row.update(clock_metrics(row, identity))
        rows.append(row)
    order = {"durability": DURABILITIES, "cohort": ORDER_COHORTS, "variant": VARIANTS}
    rows.sort(key=lambda r: (order["durability"].index(r["durability"]), order["cohort"].index(r["cohort"]),
                             r["clients"], order["variant"].index(r["variant"]), r["repeat"]))
    return identity, rows


def analyze(root, smoke=False):
    kind = "smoke" if smoke else "runs"
    identity, rows = load_rows(root, kind)
    group_list = groups(kind, identity["clients"])
    analysis = root / ("analysis-smoke" if smoke else "analysis")
    analysis.mkdir(exist_ok=False)
    table = cell_table(rows, group_list)
    checks = control_check(rows, group_list)
    write_json(analysis / "summary.json", {
        "cohort_kind": kind, "regimes": {"primary": "none", "control": "immediate"},
        "clients": identity["clients"], "groups": group_list,
        "table": table, "upstream_gate_vs_upstream": checks,
        "paired_effects": paired_effects(rows, group_list), "rows": rows,
        "failures": sum(bool(r["failure"]) for r in rows), "clock": clock_summary(rows, identity),
        "power": {k: v for k, v in (identity.get("power") or {"setup": "S0"}).items() if k != "state"},
        "caveats": ["None durability is the primary regime; Immediate (fsync inside the critical section) is a control. None is not a durable-commit claim.",
                    "service_jain is Jain over per-client service time (rdtscp ticks inside the body, charged to the requester); tx_jain counts transactions.",
                    "At 1 client every Jain index is 1 by definition; a 1-client half cohort alternates request sizes in one client.",
                    "Waiting: MCS/FC/FC-PQ spin; upstream/upstream_gate/std_mutex block (Mutex/Condvar, futex); U-SCL may sched_yield, futex-wait or nanosleep (its ban).",
                    "The critical section includes commit I/O.",
                    "Reads are outside every write lock and are not measured here.",
                    "Transaction size changes completed mix; total tx/s is not isolated lock overhead.",
                    "Smoke uses 3 repetitions: a setup/noise-bounded sanity check, not a formal result.",
                    "clock_ghz is the sampler's client mean (heuristic, whole child process; undecidable for mostly idle clients); tx_s_at_ref = tx/s x reference / clock_ghz with reference = F under S1/S2, the TSC rate under S0; clock_off_target (S1/S2) = clock outside F +- 2 %.",
                    "Close/reopen does not simulate power failure.",
                    "Transfer tx/s counts committed transfers only; aborted bodies hold the lock but return no service time."]})
    (analysis / "summary.md").write_text(markdown(table, checks))
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    fig, axes = plt.subplots(len(DURABILITIES), len(group_list), figsize=(3.6 * len(group_list), 8),
                             sharex=True, squeeze=False)
    for column, (cohort, clients) in enumerate(group_list):
        for row_index, durability in enumerate(DURABILITIES):
            axis = axes[row_index][column]
            for index, variant in enumerate(VARIANTS):
                values = [r["throughput_tx_s"] for r in rows if not r["failure"] and r["cohort"] == cohort
                          and r["clients"] == clients and r["durability"] == durability and r["variant"] == variant]
                axis.plot([index] * len(values), values, "o", alpha=.65)
            axis.set_title(f"{durability} / {cohort} / {clients}c")
            axis.set_xticks(range(len(VARIANTS)), VARIANTS, rotation=40)
            axis.set_ylabel(f"verified tx/s ({DURATION_MS // 1000}s window)")
    fig.tight_layout()
    fig.savefig(analysis / "throughput.png", dpi=160)
    plt.close(fig)
    return len([r for r in rows if r["failure"]])


def sustain_probe(out_dir, cpus, setup, fixed_ghz, seconds):
    """Spin every CPU in user mode for `seconds` (one process per CPU under its own
    perf stat) and report each CPU's effective clock = cycles / ref_tsc x TSC rate,
    plus sysfs cpuinfo_avg_freq over the run and its last 2 s. Not a trial."""
    perf = shutil.which("perf")
    if perf is None:
        raise RuntimeError("perf is required for the sustain probe")
    out_dir.mkdir(parents=True, exist_ok=True)
    stamp = time.strftime("%Y%m%dT%H%M%S")
    work = out_dir / f"sustain-{stamp}"
    work.mkdir()
    state = power_state(cpus)
    problems = check_power(state, setup, fixed_ghz)
    tsc = tsc_identity()
    tsc_ghz = tsc["hz"] / 1e9 if tsc and tsc.get("hz") else None
    if tsc_ghz is None:
        raise RuntimeError("TSC rate unknown (cpuid tool missing)")
    procs = [subprocess.Popen(["taskset", "-c", str(cpu), perf, "stat", "-x,", "-o", str(work / f"cpu{cpu}.csv"),
                               "-e", "cycles:u,cpu_clk_unhalted.ref_tsc:u", "--", "timeout", str(seconds),
                               "sh", "-c", "while :; do :; done"]) for cpu in cpus]
    begin, samples = time.monotonic(), []
    while any(p.poll() is None for p in procs):
        samples.append((time.monotonic() - begin, cpu_clocks(cpus)))
        time.sleep(0.1)
    report = []
    for index, cpu in enumerate(cpus):
        counts = {}
        for line in (work / f"cpu{cpu}.csv").read_text().splitlines():
            fields = line.split(",")
            if len(fields) > 2 and fields[0].isdigit():
                counts[fields[2]] = int(fields[0])
        cycles, ref = counts.get("cycles:u"), counts.get("cpu_clk_unhalted.ref_tsc:u")
        effective = cycles / ref * tsc_ghz if cycles and ref else None
        body = [khz[index] / 1e6 for t, khz in samples if 1.0 <= t <= seconds - 0.2 and khz[index]]
        tail = [khz[index] / 1e6 for t, khz in samples if seconds - 2.2 <= t <= seconds - 0.2 and khz[index]]
        report.append({"cpu": cpu, "effective_ghz": effective,
                       "sysfs_mean_ghz": statistics.mean(body) if body else None,
                       "sysfs_min_ghz": min(body) if body else None,
                       "sysfs_last2s_mean_ghz": statistics.mean(tail) if tail else None})
    target = fixed_ghz if setup in ("S1", "S2") else None
    held = None if target is None else all(
        r["effective_ghz"] and abs(r["effective_ghz"] / target - 1) <= CLOCK_TOLERANCE
        and r["sysfs_last2s_mean_ghz"] and abs(r["sysfs_last2s_mean_ghz"] / target - 1) <= CLOCK_TOLERANCE
        for r in report)
    load = os.getloadavg()
    result = {"setup": setup, "fixed_ghz": target, "seconds": seconds, "tsc_ghz": tsc_ghz, "cpus": list(cpus),
              "power_problems": problems, "power_state": state, "loadavg": load, "per_cpu": report,
              "held_within_tolerance": held, "tolerance": CLOCK_TOLERANCE}
    write_json(work / "sustain.json", result)
    for r in report:
        print(f"cpu{r['cpu']}: perf {r['effective_ghz']:.3f} GHz, sysfs mean {r['sysfs_mean_ghz']:.3f} "
              f"min {r['sysfs_min_ghz']:.3f} last-2s {r['sysfs_last2s_mean_ghz']:.3f}")
    print(f"setup {setup} F={target}: held={held}; power mismatches: {len(problems)}; "
          f"loadavg {load[0]:.2f}; {work / 'sustain.json'}")
    return held


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--prepare-only", action="store_true")
    mode.add_argument("--smoke", action="store_true")
    mode.add_argument("--run", action="store_true", help="formal matrix; only when explicitly intended")
    mode.add_argument("--analyze-only", action="store_true", help="analyze the formal matrix")
    mode.add_argument("--analyze-smoke", action="store_true", help="analyze the smoke cohort")
    mode.add_argument("--perf", action="store_true", help="cache-counter profile cohort under perf stat")
    mode.add_argument("--analyze-perf", action="store_true", help="analyze the perf profile cohort")
    mode.add_argument("--sustain-probe", action="store_true",
                      help="spin every --cpus CPU and report its effective clock (output under --output-root)")
    mode.add_argument("--check-power", action="store_true",
                      help="print mismatches between the host and --power-setup/--fixed-ghz; exit 1 on any")
    parser.add_argument("--output-root", type=Path)
    parser.add_argument("--build-dir", type=Path, default=OUT, help="verified build.py output (prepare)")
    parser.add_argument("--cpus", type=parse_cpus,
                        help="distinct CPUs, at least the largest client count (8); default at prepare: 8-15")
    parser.add_argument("--numa-node", type=parse_numa_node, default=argparse.SUPPRESS,
                        help="memory node for numactl; default at prepare: 0; none disables binding")
    parser.add_argument("--power-setup", choices=POWER_SETUPS, default=argparse.SUPPRESS,
                        help="S0 stock, S1 fixed clock, S2 fixed clock + C6 off on the run CPUs "
                             "(required at prepare; must match the manifest when given later)")
    parser.add_argument("--fixed-ghz", type=float, default=DEFAULT_FIXED_GHZ,
                        help=f"S1/S2 target clock F in GHz (default {DEFAULT_FIXED_GHZ})")
    parser.add_argument("--variants", default=",".join(VARIANTS),
                        help="comma-separated variant subset (prepare only; default all)")
    parser.add_argument("--probe-seconds", type=float, default=10.0)
    args = parser.parse_args()
    setup = getattr(args, "power_setup", None)
    if args.check_power or args.sustain_probe:
        cpus = args.cpus if args.cpus is not None else DEFAULT_CPUS
        if setup is None:
            raise RuntimeError("--power-setup is required")
        if args.check_power:
            problems = check_power(power_state(cpus), setup, args.fixed_ghz)
            print("\n".join(problems) if problems else f"host matches {setup}"
                  + (f" at {args.fixed_ghz} GHz" if setup != "S0" else ""))
            if problems:
                raise RuntimeError(f"{len(problems)} power mismatches")
            return
        if args.output_root is None:
            raise RuntimeError("--output-root is required")
        sustain_probe(args.output_root.resolve(), cpus, setup, args.fixed_ghz, args.probe_seconds)
        return
    if args.output_root is None:
        raise RuntimeError("--output-root is required")
    root = args.output_root.resolve()
    if args.prepare_only:
        cpus = args.cpus if args.cpus is not None else DEFAULT_CPUS
        numa_node = getattr(args, "numa_node", 0)
        if setup is None:
            raise RuntimeError("--power-setup is required at prepare (S0 = stock host)")
        variants = [v for v in args.variants.split(",") if v]
        if not variants or any(v not in VARIANTS for v in variants) or len(set(variants)) != len(variants):
            raise RuntimeError(f"--variants must be distinct names from {VARIANTS}")
        prepare(root, args.build_dir.resolve(), cpus, numa_node, setup, args.fixed_ghz, variants)
    elif args.smoke or args.run or args.perf:
        identity = load_identity(root)
        if args.cpus is not None and list(args.cpus) != identity["cpus"]:
            raise RuntimeError("--cpus differs from prepared campaign")
        if hasattr(args, "numa_node") and args.numa_node != identity["numa_memory_node"]:
            raise RuntimeError("--numa-node differs from prepared campaign")
        if setup is not None and setup != identity["power"]["setup"]:
            raise RuntimeError("--power-setup differs from prepared campaign")
        run_matrix(root, "smoke" if args.smoke else "perf" if args.perf else "runs")
    elif args.analyze_perf:
        if analyze_perf(root):
            raise RuntimeError("perf analysis contains failed cells; inspect raw results")
    elif analyze(root, smoke=args.analyze_smoke):
        raise RuntimeError("analysis contains failed cells; inspect raw results")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"redb campaign: {error}", file=sys.stderr)
        sys.exit(1)
