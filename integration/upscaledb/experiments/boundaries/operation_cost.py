#!/usr/bin/env python3
"""Fixed-computation cost boundary: 48 primary trials + 18 separate diagnostics.

Launch prepare/analyze under the assigned shared-global/exclusive-slot wrapper;
launch --run under the exclusive-global/exclusive-slot wrapper.
All children inherit the controller's CPU mask and externally held locks. No DB or
Rust rebuild, concurrent measurement, resume/overwrite, or favorable-case selection.
"""
import argparse
import csv
import hashlib
import json
import math
import os
from pathlib import Path
import random
import shutil
import signal
import statistics as stats
import subprocess
import sys

from integration.upscaledb._paths import CORE, ROOT, RUNNER, UPSCALEDB
from integration.upscaledb.core.build import rust_sources_digest
from integration.upscaledb.runner.run_trials import discover_topology
from integration.upscaledb.runner.process_execution import capture_command, utc_now

HERE = Path(__file__).resolve().parent
FROZEN = ROOT / '.worktree/upscaledb'
EXPERIMENT = 'cost_asymmetry_boundary'
CPUS = list(range(20, 28))
BACKENDS = ('bridge_mutex', 'fc', 'fc_pq', 'uscl')
PROFILE_BACKENDS = ('fc', 'fc_pq', 'uscl')
CASES = {'one_equal': (1, 1), 'eight_equal': (8, 8),
         'four_four': (8, 4), 'seven_one': (8, 7)}
HETEROGENEOUS = ('four_four', 'seven_one')
REPETITIONS = 3
TIMEOUT = 15
NOTES = [
    'Synthetic fixed dependent integer/memory callbacks, not database or production throughput.',
    '64 versus 1024 iterations; identical fixed shared-state footprint in every case. The lookup table is read-only; protected requester counters and checksums mutate on each call.',
    'No assumed winner: one/eight equal-cheap controls may expose delegation/scheduling cost; heterogeneity may redistribute service while lowering operation throughput.',
    'No warmup or retuning: fresh processes, gate-started closed-loop requesters, first-use registration included. Primary2s and diagnostic1s windows are exploratory.',
    'Expected callback result precomputed once per worker; per-return O(1) checks do not recompute the expensive work outside the lock.',
    'Primary callback has no added service clock or atomic nonoverlap guard. Requester monotonic timestamps and fixed histograms add common but not zero overhead.',
    'Nonoverlap atomic guard and injected application errors run only in deterministic fixed-count preparation smoke, not primary or diagnostic timing.',
    'Primary means uninstrumented frozen bridge, not an overhead-free harness. Error/status/count/checksum checks remain in every timed run.',
    'A successful bridge return publishes requester-owned stack output; all ABI callers and USCL TLS destructors are joined before exclusive destruction.',
    'Before-deadline counts require the synchronous return by deadline. A callback may have finished earlier but returned during drain; callback occupancy at cutoff is NOT measured.',
    'Closed-loop worker submits only while request timestamp precedes deadline, at most one pending request per worker; drain counts are reported, not hidden.',
    'Requester CPU covers active work including any drain, and process CPU covers gate-to-join. Neither is falsely labeled an exact deadline-window CPU sample.',
    'Profile service is elapsed callback time attributed to the original requester, not CPU cycles, scheduler virtual usage, or operation-count fairness.',
    'Profile before-deadline service uses response-completion attribution, not exact service-window clipping. Drain service is separate; executor totals cover complete calls.',
    'Profile/primary rate ratios disclose perturbation PLUS different1s/2s windows and first-use effects; they do not isolate pure instrumentation overhead.',
    'Fixed-iteration units/s counts completed synthetic loop iterations; it is not measured service time or a substitute for either role throughput.',
    'Three fresh-process repetitions per cell: show all points and observed ranges, never confidence intervals, significance or equivalence.',
    'Predeclared primary per-metric rule: all three paired ratios>1.05 increase; all<0.95 decrease; otherwise mixed/small, NOT equivalence. Higher throughput and lower CPU/latency orient differently.',
    'Usage-fairness evidence uses service Jain and role shares only. Throughput comparisons do not diagnose cache coherence, idle-with-pending work, or starvation.',
    'CPUs20-27 are distinct physical cores on node0, reserved SMT84-91 unused. Cooperative locks exclude cohort overlap, not unrelated host/cache/power noise.',
]


def sha(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def save(path, value):
    with Path(path).open('x') as stream:
        json.dump(value, stream, indent=2, allow_nan=False)
        stream.write('\n')


def load(path):
    return json.loads(Path(path).read_text())


def require(okay, message):
    if not okay:
        raise ValueError(message)


def affinity():
    actual = sorted(os.sched_getaffinity(0))
    require(actual == CPUS, f'launch under taskset -c20-27; inherited mask is {actual}')
    return actual


def sources_to_archive():
    # Exact source set of the existing build.rust_sources_digest convention,
    # plus the controller/harness/helpers. Archive bytes, not only a git label.
    files = [ROOT / 'Cargo.toml', ROOT / 'Cargo.lock', UPSCALEDB / '_paths.py',
             CORE / 'bridge.h', HERE / 'operation_cost.cc', Path(__file__).resolve(),
             CORE / 'build.py', RUNNER / 'run_trials.py', RUNNER / 'process_execution.py']
    files.extend((ROOT / '.cargo').glob('*.toml'))
    for suffix in ('*.c', '*.h'):
        files.extend((ROOT / 'c').rglob(suffix))
    for crate in ('libdlock', 'upscaledb-bridge'):
        directory = ROOT / 'crates' / crate
        files.append(directory / 'Cargo.toml')
        files.extend(directory.glob('src/**/*.rs'))
        files.extend(directory.glob('binding/**/*.h'))
        if (directory / 'build.rs').is_file():
            files.append(directory / 'build.rs')
    return sorted(set(files))


def schedule(seed):
    rng = random.Random(seed)
    cells = []
    for kind, names, cases, ms in [('primary', BACKENDS, tuple(CASES), 2000),
                                    ('diagnostic', PROFILE_BACKENDS, HETEROGENEOUS, 1000)]:
        for rep in range(REPETITIONS):
            order = list(cases)
            rng.shuffle(order)
            for case in order:
                backends = list(names)
                rng.shuffle(backends)
                for backend in backends:
                    cells.append({'index': len(cells), 'kind': kind, 'case': case,
                                  'backend': backend, 'repetition': rep,
                                  'variant': backend + ('_profile' if kind == 'diagnostic' else ''),
                                  'duration_ms': ms})
    return cells


def command_for(manifest, cell, smoke=False):
    return [manifest['binaries'][cell['variant']], cell['backend'], cell['case'],
            'smoke' if smoke else 'timed', str(cell['duration_ms'])]


def assert_inputs(manifest):
    require(manifest['schedule'] == schedule(manifest['seed']) and
            manifest['expected_trials'] == 66, 'predefined schedule changed')
    for path, expected in manifest['files'].items():
        require(sha(path) == expected, f'input identity changed: {path}')
    require(rust_sources_digest() == manifest['rust_sources_sha256'], 'frozen Rust/C source drift')


def parse_record(record, cell, smoke=False):
    try:
        require(record['returncode'] == 0 and not record['timeout'], 'process failed/timed out')
        objects = [json.loads(line) for line in record['stdout'].splitlines() if line.strip()]
        setup = [o for o in objects if o.get('type') == 'setup']
        end = [o for o in objects if o.get('type') == 'complete']
        workers = [o for o in objects if o.get('type') == 'worker']
        count, cheap = CASES[cell['case']]
        require(len(setup) == len(end) == 1 and len(workers) == count and
                len(objects) == count + 2, 'missing/duplicate output records')
        setup, end = setup[0], end[0]
        require(setup['backend'] == cell['backend'] and setup['case'] == cell['case'] and
                setup['profile'] == (cell['kind'] == 'diagnostic') and
                setup['smoke'] == smoke and setup['nonoverlap_guard'] == smoke,
                'binary/case/mode instrumentation mismatch')
        require(setup['workers'] == count and setup['cheap_workers'] == cheap and
                setup['cheap_iterations'] == 64 and setup['expensive_iterations'] == 1024,
                'workload mismatch')
        require(setup['inherited_affinity'] == CPUS and setup['memory_node_mask'] == 1 and
                setup['protected_page_initial_node'] == end['protected_page_final_node'] == 0,
                'observed CPU/NUMA placement mismatch')
        require(end['status'] == 'ok' and end['joined_before_destroy'] and
                end['deadline_ns'] - end['start_ns'] == end['window_ns'] == cell['duration_ms'] * 1000000,
                'completion/lifetime/window oracle')
        require(end['calls'] == sum(w['calls'] for w in workers) and
                end['before_deadline'] == sum(w['before_deadline'] for w in workers) and
                end['drain'] == sum(w['drain'] for w in workers), 'aggregate count conservation')
        require([w['id'] for w in workers] == list(range(count)), 'stable worker identities')
        for w in workers:
            cpu = 20 + w['id']
            require(w['actual_cpu_start'] == w['actual_cpu_end'] == cpu and
                    w['actual_affinity'] == [cpu], 'observed worker pin mismatch')
            require(w['role'] == ('cheap' if w['id'] < cheap else 'expensive') and
                    w['iterations'] == (64 if w['id'] < cheap else 1024), 'worker cost identity')
            require(w['calls'] == w['before_deadline'] + w['drain'] and
                    w['application_errors'] == (2 if smoke else 0), 'worker count/error oracle')
            require((w['calls'] == 32 if smoke else w['drain'] <= 1), 'bounded drain/fixed smoke')
            for name, total in [('response_latency_before_deadline', w['before_deadline']),
                                ('response_latency_drain', w['drain'])]:
                hist = w[name]['upper_bound_log2_counts']
                require(len(hist) == 64 and all(isinstance(x, int) and x >= 0 for x in hist) and
                        sum(hist) == total, 'latency histogram conservation')
            require(len(w['progress_bins']) == 20 and
                    sum(w['progress_bins']) == (0 if smoke else w['before_deadline']),
                    'completion progress-bin conservation')
        if setup['profile']:
            require(end['profile_conservation_checked'] and
                    sum(w['executor_callbacks'] for w in workers) == end['calls'] and
                    sum(w['executor_service_ns'] for w in workers) ==
                    sum(w['service_before_deadline_ns'] + w['service_drain_ns'] for w in workers),
                    'profile attribution conservation')
        else:
            require(all(w['service_before_deadline_ns'] == w['service_drain_ns'] == 0 and
                        w['executor_callbacks'] == 0 for w in workers), 'primary must not be profiled')
        return {'setup': setup, 'complete': end, 'workers': workers}, None
    except (KeyError, ValueError, TypeError, json.JSONDecodeError) as exc:
        return None, str(exc) or type(exc).__name__


def prepare(args):
    affinity()
    root = args.output_root
    root.mkdir(parents=True, exist_ok=False)
    topology = discover_topology()
    selected = [topology['cpus'][str(cpu)] for cpu in CPUS]
    require(all(c['node'] == 0 for c in selected) and
            len({(c['socket'], c['core']) for c in selected}) == 8,
            'slot must be eight distinct physical cores on node0')
    files, snapshots = {}, {}
    for source in sources_to_archive():
        destination = root / 'src' / source.relative_to(ROOT)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        files[str(source)] = files[str(destination)] = sha(source)
        snapshots[str(source.relative_to(ROOT))] = str(destination.relative_to(root))
    if args.resource_registry is not None:
        resources = args.resource_registry.resolve(strict=True)
        slot = load(resources)['slots']['CostBoundary']
        require(slot['cpus'] == '20-27' and slot['numa_node'] == 0 and
                slot['reserved_siblings'] == '84-91', 'resource registry allocation mismatch')
        shutil.copy2(resources, root / 'allocations.json')
        files[str(root / 'allocations.json')] = sha(root / 'allocations.json')
    source = root / 'src/integration/upscaledb/experiments/boundaries/operation_cost.cc'
    rust_hash = rust_sources_digest()
    variants = list(BACKENDS) + [b + '_profile' for b in PROFILE_BACKENDS]
    binaries, build_commands, frozen_manifests = {}, {}, {}
    (root / 'bin').mkdir()
    env = os.environ.copy()
    env.pop('NIX_CFLAGS_COMPILE', None)
    env.pop('NIX_LDFLAGS', None)
    save(root / 'prepare-start.json', {'created_utc': utc_now(), 'command': sys.argv,
         'cwd': str(ROOT), 'topology': topology, 'expected_primary_trials': 48,
         'expected_diagnostic_trials': 18, 'notes': NOTES})
    save(root / 'host.json', {'created_utc': utc_now(), 'uname': list(os.uname()),
         'python': sys.version, 'cpuinfo': Path('/proc/cpuinfo').read_text(),
         'numa_balancing': Path('/proc/sys/kernel/numa_balancing').read_text().strip(),
         'scheduler_policy': os.sched_getscheduler(0), 'nice': os.getpriority(os.PRIO_PROCESS, 0),
         'cpus': affinity(), 'topology': topology})
    files[str(root / 'host.json')] = sha(root / 'host.json')
    for variant in variants:
        path = args.binary_root / f'build-{variant}.json'
        entry = load(path)
        require(entry['variant'] == variant, 'frozen variant mismatch')
        frozen_manifests[variant] = entry
        shutil.copy2(path, root / path.name)
        files[str(path)] = files[str(root / path.name)] = sha(path)
        library = entry['rust_staticlib']
        require(library['rust_sources_sha256'] == rust_hash and
                sha(CORE / 'bridge.h') == library['bridge_h_sha256'], 'frozen Rust source/ABI mismatch')
        expected_profile = variant.endswith('_profile')
        require(('profile' in library['features']) == expected_profile, 'primary/profile archive mismatch')
        dependencies = [(library['archive'], library['archive_sha256']),
                        (entry['library_verification']['shared_object'],
                         entry['library_verification']['shared_object_sha256']),
                        (entry['toolchain']['cxx'], entry['toolchain']['cxx_sha256'])]
        for dep, expected in dependencies:
            require(sha(dep) == expected, f'frozen artifact changed: {dep}')
            files[dep] = expected
        binary = root / 'bin' / ('cost-' + variant)
        binaries[variant] = str(binary)
        command = list(entry['build']['harness_command'])
        old = [s for s in command if s.endswith('/native_harness.cc')]
        require(len(old) == 1 and command.count('-o') == 1, 'unexpected frozen compiler command')
        command[command.index(old[0])] = str(source)
        command[command.index('-o') + 1] = str(binary)
        build_commands[variant] = command
        result = capture_command(command, 180, cwd=ROOT, env=env,
                                 metadata={'cwd': str(ROOT),
                                           'parent_affinity': sorted(os.sched_getaffinity(0))})
        result['environment_removed'] = ['NIX_CFLAGS_COMPILE', 'NIX_LDFLAGS']
        save(root / f'compile-{variant}.json', result)
        require(result['returncode'] == 0 and not result['timeout'], f'{variant} compile failed; logs retained')
        files[str(binary)] = sha(binary)
    cells = schedule(args.seed)
    manifest = {'schema': 1, 'experiment': EXPERIMENT, 'created_utc': utc_now(),
                'source_git_head': subprocess.check_output(
                    ['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                'scope': 'synthetic cost asymmetry; not DB throughput',
                'source_root': str(ROOT), 'binary_root': str(args.binary_root), 'files': files,
                'source_snapshots': snapshots, 'rust_sources_sha256': rust_hash,
                'binaries': binaries, 'build_commands': build_commands,
                'frozen_uscl_baseline': frozen_manifests['uscl']['baseline'],
                'backends': BACKENDS, 'profile_backends': PROFILE_BACKENDS,
                'cases': {name: {'workers': values[0], 'cheap_workers': values[1]}
                          for name, values in CASES.items()},
                'seed': args.seed, 'repetitions': REPETITIONS, 'primary_window_ms': 2000,
                'diagnostic_window_ms': 1000, 'timeout_s': TIMEOUT,
                'expected_primary_trials': 48, 'expected_diagnostic_trials': 18,
                'expected_trials': 66, 'schedule': cells, 'topology': topology,
                'cpus': CPUS, 'reserved_siblings': list(range(84, 92)), 'memory_node': 0,
                'notes': NOTES, 'smokes': []}
    seen = set()
    for cell in cells:
        identity = (cell['variant'], cell['case'])
        if identity in seen:
            continue
        seen.add(identity)
        result = capture_command(command_for(manifest, cell, smoke=True), TIMEOUT, cwd=ROOT,
                                 metadata={'cwd': str(ROOT),
                                           'parent_affinity': sorted(os.sched_getaffinity(0))})
        parsed, error = parse_record(result, cell, smoke=True)
        result.update(validation_error=error, cell=cell, parsed=parsed)
        path = root / f'smoke-{cell["variant"]}-{cell["case"]}.json'
        save(path, result)
        manifest['smokes'].append({'file': path.name, 'sha256': sha(path), 'error': error})
    require(len(seen) == 22 and all(s['error'] is None for s in manifest['smokes']),
            'fixed-count smoke/oracle failed; all attempted smokes retained')
    assert_inputs(manifest)
    save(root / 'prepare.json', manifest)
    print(f'Prepared {root}: 48 primary +18 diagnostic fresh-process trials; 22 fixed-count smokes.')


def run(args):
    affinity()
    root = args.output_root
    manifest = load(root / 'prepare.json')
    assert_inputs(manifest)
    for smoke in manifest['smokes']:
        require(smoke['error'] is None and sha(root / smoke['file']) == smoke['sha256'], 'smoke changed')
    raw = root / 'trials'
    raw.mkdir(exist_ok=False)
    save(root / 'run-start.json', {'started_utc': utc_now(), 'command': sys.argv, 'cwd': str(ROOT),
         'prepare_sha256': sha(root / 'prepare.json'), 'parent_affinity': affinity(),
         'topology': discover_topology(), 'schedule': manifest['schedule'],
         'cpu_stat_before': Path('/proc/stat').read_text(),
         'pressure_cpu_before': Path('/proc/pressure/cpu').read_text()})
    records = []
    for cell in manifest['schedule']:
        record = capture_command(command_for(manifest, cell), manifest['timeout_s'], cwd=ROOT,
                                 metadata={'cwd': str(ROOT),
                                           'parent_affinity': sorted(os.sched_getaffinity(0))})
        _, error = parse_record(record, cell)
        record.update(cell=cell, validation_error=error,
                      prepare_sha256=sha(root / 'prepare.json'))
        path = raw / f'{cell["index"]:03d}.json'
        save(path, record)
        records.append({**cell, 'file': str(path.relative_to(root)), 'sha256': sha(path), 'error': error})
    run_record = {'finished_utc': utc_now(), 'trials': records,
                  'prepare_sha256': sha(root / 'prepare.json'),
                  'cpu_stat_after': Path('/proc/stat').read_text(),
                  'pressure_cpu_after': Path('/proc/pressure/cpu').read_text()}
    try:
        assert_inputs(manifest)
        run_record['input_error'] = None
    except (OSError, ValueError) as exc:
        run_record['input_error'] = str(exc)
    save(root / 'run.json', run_record)
    failures = sum(r['error'] is not None for r in records)
    print(f'Collected {len(records)}/66 processes; failures={failures}; analysis is separate.')
    return bool(failures or run_record['input_error'])


def percentile(workers, quantile):
    histogram = [sum(w['response_latency_before_deadline']['upper_bound_log2_counts'][i]
                     for w in workers) for i in range(64)]
    total = sum(histogram)
    if not total:
        return None
    target, cumulative = math.ceil(total * quantile), 0
    for index, count in enumerate(histogram):
        cumulative += count
        if cumulative >= target:
            return (1 << index) / 1000  # Log2-bin upper bound, in microseconds.


def jain(values):
    denominator = len(values) * sum(v * v for v in values)
    return sum(values) ** 2 / denominator if denominator else None


def metrics(parsed):
    workers, end = parsed['workers'], parsed['complete']
    seconds = end['window_ns'] / 1e9
    before = end['before_deadline']
    cpu = sum(w['cpu_ns_active_including_drain'] for w in workers) / 1e9
    result = {'operations_per_s': before / seconds,
              'synthetic_iteration_units_per_s': sum(w['before_deadline'] * w['iterations'] for w in workers) / seconds,
              'before_deadline_operations': before, 'drain_operations': end['drain'],
              'drain_wall_ms': end['drain_wall_ns'] / 1e6,
              'worker_cpu_s_including_drain': cpu,
              'process_cpu_s_gate_to_join': end['process_cpu_ns_gate_to_join'] / 1e9,
              'cpu_us_including_drain_per_ontime_op': cpu * 1e6 / before if before else None,
              'cpu_ns_including_drain_per_iteration_unit': cpu * 1e9 /
                  sum(w['before_deadline'] * w['iterations'] for w in workers) if before else None,
              'response_p50_upper_us': percentile(workers, .5),
              'response_p99_upper_us': percentile(workers, .99),
              'requester_min_operations_per_s': min(w['before_deadline'] for w in workers) / seconds,
              'requester_max_operations_per_s': max(w['before_deadline'] for w in workers) / seconds,
              'max_completion_gap_ms_including_drain': max(w['max_completion_gap_ns_including_drain'] for w in workers) / 1e6,
              'max_start_lag_us': max(w['started_ns'] - end['start_ns'] for w in workers) / 1000}
    service = [w['service_before_deadline_ns'] for w in workers]
    total_service = sum(service)
    for role in ('cheap', 'expensive'):
        role_workers = [w for w in workers if w['role'] == role]
        if not role_workers:
            continue
        calls = sum(w['before_deadline'] for w in role_workers)
        result.update({role + '_operations_per_s': calls / seconds,
                       role + '_mean_worker_operations_per_s': calls / seconds / len(role_workers),
                       role + '_min_worker_operations_per_s': min(w['before_deadline'] for w in role_workers) / seconds,
                       role + '_response_p99_upper_us': percentile(role_workers, .99)})
        if parsed['setup']['profile']:
            role_service = sum(w['service_before_deadline_ns'] for w in role_workers)
            result.update({role + '_service_share': role_service / total_service if total_service else None,
                           role + '_equal_weight_reference': len(role_workers) / len(workers),
                           role + '_service_mean_ns': role_service / calls if calls else None,
                           role + '_service_jain': jain([w['service_before_deadline_ns'] for w in role_workers])})
    if parsed['setup']['profile']:
        result.update(service_jain=jain(service), service_before_deadline_ns=total_service,
                      service_drain_ns=sum(w['service_drain_ns'] for w in workers),
                      cheap_service_share_error=(abs(result['cheap_service_share'] - result['cheap_equal_weight_reference'])
                                                 if total_service else None))
        cheap, expensive = result.get('cheap_service_mean_ns'), result.get('expensive_service_mean_ns')
        result['expensive_cheap_service_ratio'] = expensive / cheap if cheap and expensive is not None else None
    return result


def spread(values):
    values = [x for x in values if x is not None]
    return {'n': len(values), 'values': values, 'mean': stats.mean(values) if values else None,
            'min': min(values) if values else None, 'max': max(values) if values else None}


def verdict(ratios):
    if len(ratios) != 3 or any(x is None for x in ratios):
        return 'insufficient_evidence'
    if min(ratios) > 1.05:
        return 'consistent_increase'
    if max(ratios) < .95:
        return 'consistent_decrease'
    return 'mixed_or_small_not_equivalence'


PRIMARY_METRICS = {
    'operations_per_s': 'higher', 'synthetic_iteration_units_per_s': 'higher',
    'cpu_us_including_drain_per_ontime_op': 'lower',
    'cpu_ns_including_drain_per_iteration_unit': 'lower',
    'response_p99_upper_us': 'lower', 'cheap_mean_worker_operations_per_s': 'higher',
    'expensive_mean_worker_operations_per_s': 'higher', 'cheap_response_p99_upper_us': 'lower',
    'expensive_response_p99_upper_us': 'lower',
}


def contrasts(rows):
    index = {(r['kind'], r['case'], r['backend'], r['repetition']): r['metrics']
             for r in rows if r['error'] is None}
    primary, fairness, perturbation = [], [], []
    for case in CASES:
        for baseline in ('fc', 'bridge_mutex', 'uscl'):
            for metric, favorable in PRIMARY_METRICS.items():
                if metric.startswith('expensive_') and case not in HETEROGENEOUS:
                    continue
                pairs = []
                for rep in range(3):
                    a = index.get(('primary', case, 'fc_pq', rep), {}).get(metric)
                    b = index.get(('primary', case, baseline, rep), {}).get(metric)
                    pairs.append({'repetition': rep, 'fc_pq': a, 'baseline': b,
                                  'ratio': a / b if a is not None and b else None})
                ratios = [p['ratio'] for p in pairs]
                primary.append({'case': case, 'comparator': baseline, 'metric': metric,
                                'favorable_direction': favorable, 'paired_points': pairs,
                                'ratio_spread': spread(ratios), 'classification': verdict(ratios)})
    for case in HETEROGENEOUS:
        for baseline in ('fc', 'uscl'):
            points = []
            for rep in range(3):
                a = index.get(('diagnostic', case, 'fc_pq', rep), {})
                b = index.get(('diagnostic', case, baseline, rep), {})
                good = all(x.get(k) is not None for x in (a, b)
                           for k in ('service_jain', 'cheap_service_share_error'))
                points.append({'repetition': rep,
                               'service_jain_increase': a['service_jain'] - b['service_jain'] if good else None,
                               'role_share_error_reduction': b['cheap_service_share_error'] - a['cheap_service_share_error'] if good else None})
            complete = all(p['service_jain_increase'] is not None for p in points)
            positive = complete and all(p['service_jain_increase'] > 0 and p['role_share_error_reduction'] > 0 for p in points)
            negative = complete and all(p['service_jain_increase'] < 0 and p['role_share_error_reduction'] < 0 for p in points)
            fairness.append({'case': case, 'comparator': baseline, 'paired_points': points,
                             'classification': ('insufficient_evidence' if not complete else
                                                'both_measures_improve_all_three' if positive else
                                                'both_measures_worsen_all_three' if negative else
                                                'mixed_or_small_not_equivalence')})
        for backend in PROFILE_BACKENDS:
            points = []
            for rep in range(3):
                a = index.get(('diagnostic', case, backend, rep), {}).get('operations_per_s')
                b = index.get(('primary', case, backend, rep), {}).get('operations_per_s')
                points.append({'repetition': rep, 'profile_1s_ops_s': a, 'primary_2s_ops_s': b,
                               'profile_over_primary_ratio': a / b if a is not None and b else None})
            perturbation.append({'case': case, 'backend': backend, 'paired_points': points,
                                 'ratio_spread': spread([p['profile_over_primary_ratio'] for p in points]),
                                 'caveat': 'Instrumentation plus1s/2s-window/first-use differences; not pure overhead.'})
    return primary, fairness, perturbation


def write_csv(path, rows):
    with path.open('x', newline='') as stream:
        if rows:
            writer = csv.DictWriter(stream, fieldnames=list(rows[0]))
            writer.writeheader()
            writer.writerows(rows)


def plots(out, rows):
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    colors = {'bridge_mutex': '#555555', 'fc': '#1f77b4', 'fc_pq': '#d62728', 'uscl': '#2ca02c'}
    primary = [r for r in rows if r['kind'] == 'primary' and r['error'] is None]
    diagnostics = [r for r in rows if r['kind'] == 'diagnostic' and r['error'] is None]

    def points(ax, candidates, metric, label, cases, scale=1):
        for case_idx, case in enumerate(cases):
            for b_idx, backend in enumerate(BACKENDS):
                group = [r for r in candidates if r['case'] == case and r['backend'] == backend]
                values = [r['metrics'].get(metric) for r in group]
                values = [v / scale for v in values if v is not None]
                x = case_idx + (b_idx - 1.5) * .17
                if values:
                    ax.plot([x, x], [min(values), max(values)], color=colors[backend], lw=1)
                    ax.scatter([x + (i - 1) * .024 for i in range(len(values))], values,
                               color=colors[backend], s=23, label=backend if case_idx == 0 else None)
        ax.set_xticks(range(len(cases)), cases, rotation=12)
        ax.set_ylabel(label)
        ax.grid(axis='y', alpha=.2)

    outputs = []
    for filename, title, specs in [
        ('primary-rates.png', 'Primary uninstrumented bridge: all three points, observed ranges',
         [('operations_per_s', 'Completed operations/s (millions)', 1e6),
          ('synthetic_iteration_units_per_s', 'Synthetic iterations/s (billions; NOT service time)', 1e9)]),
        ('primary-cpu-latency.png', 'Requester CPU includes drain; latency is log2-bin upper bound',
         [('cpu_us_including_drain_per_ontime_op', 'Worker CPU us / before-deadline operation', 1),
          ('response_p99_upper_us', 'p99 response latency upper bound (us)', 1)]),
        ('primary-role-progress.png', 'Per-role mean requester progress; NOT usage fairness',
         [('cheap_mean_worker_operations_per_s', 'Cheap operations/s per requester', 1),
          ('expensive_mean_worker_operations_per_s', 'Expensive operations/s per requester', 1)])]:
        fig, axes = plt.subplots(1, 2, figsize=(13, 4.5))
        for ax, (metric, label, scale) in zip(axes, specs):
            points(ax, primary, metric, label, list(CASES), scale)
        axes[0].legend(fontsize=8)
        fig.suptitle(title); fig.tight_layout()
        fig.savefig(out / filename, dpi=170); plt.close(fig); outputs.append(filename)
    fig, axes = plt.subplots(1, 2, figsize=(11, 4.5))
    points(axes[0], diagnostics, 'service_jain', 'Requester callback-service Jain', list(HETEROGENEOUS))
    points(axes[1], diagnostics, 'cheap_service_share', 'Cheap-role elapsed-service share', list(HETEROGENEOUS))
    for i, case in enumerate(HETEROGENEOUS):
        reference = CASES[case][1] / 8
        axes[1].plot([i - .4, i + .4], [reference, reference], 'k--', lw=1)
    axes[0].set_ylim(0, 1.03); axes[1].set_ylim(0, 1.03); axes[0].legend(fontsize=8)
    fig.suptitle('Separate1s instrumented service allocation; dashed lines: equal-worker role share')
    fig.tight_layout(); fig.savefig(out / 'diagnostic-service-allocation.png', dpi=170)
    plt.close(fig); outputs.append('diagnostic-service-allocation.png')
    return outputs


def analyze(args):
    root = args.output_root
    manifest = load(root / 'prepare.json')
    # Analysis consumes archived data only: no source-tree/binary rehash, build,
    # subprocess or benchmark. Missing trials remain explicit failures.
    out = args.analysis_root or root / 'analysis'
    out.mkdir(parents=True, exist_ok=False)
    preparation_hash = sha(root / 'prepare.json')
    integrity_errors = []
    require(manifest['schedule'] == schedule(manifest['seed']) and
            len(manifest['schedule']) == 66, 'saved predefined schedule changed')
    for original, relative in manifest['source_snapshots'].items():
        archived = root / relative
        expected = manifest['files'][str(archived)]
        if not archived.is_file() or sha(archived) != expected:
            integrity_errors.append('archived source identity changed: ' + original)
    saved_run = load(root / 'run.json') if (root / 'run.json').exists() else None
    if saved_run is None:
        integrity_errors.append('run.json missing: collection incomplete/interrupted')
    elif saved_run['prepare_sha256'] != preparation_hash or saved_run.get('input_error'):
        integrity_errors.append(saved_run.get('input_error') or 'run/prepare identity mismatch')
    expected_hashes = {r['file']: r['sha256'] for r in saved_run['trials']} if saved_run else {}
    rows = []
    for cell in manifest['schedule']:
        path = root / 'trials' / f'{cell["index"]:03d}.json'
        row = {**cell, 'raw_file': str(path.relative_to(root)), 'error': None, 'metrics': {}, 'observations': None}
        try:
            record = load(path)
            row['raw_sha256'] = sha(path)
            require(record['cell'] == cell and record['prepare_sha256'] == preparation_hash,
                    'raw trial schedule/prepare mismatch')
            if saved_run:
                require(expected_hashes.get(row['raw_file']) == row['raw_sha256'], 'raw file hash mismatch')
            parsed, error = parse_record(record, cell)
            require(error is None, error)
            require(record.get('validation_error') is None, 'collector marked trial failed')
            row['metrics'] = metrics(parsed)
            row['observations'] = parsed
        except (OSError, ValueError, KeyError, TypeError) as exc:
            row['error'] = str(exc) or type(exc).__name__
        rows.append(row)
    groups = []
    for kind, names, cases in [('primary', BACKENDS, CASES),
                                ('diagnostic', PROFILE_BACKENDS, HETEROGENEOUS)]:
        for case in cases:
            for backend in names:
                selected = [r for r in rows if (r['kind'], r['case'], r['backend']) == (kind, case, backend)]
                valid = [r for r in selected if r['error'] is None]
                keys = sorted({k for r in valid for k in r['metrics']})
                groups.append({'kind': kind, 'case': case, 'backend': backend,
                               'expected': 3, 'successful': len(valid), 'failed': 3 - len(valid),
                               'metrics': {key: spread([r['metrics'].get(key) for r in valid]) for key in keys}})
    primary, fairness, perturbation = contrasts(rows)
    counts = {}
    for kind, expected in [('primary', 48), ('diagnostic', 18)]:
        valid = sum(r['kind'] == kind and r['error'] is None for r in rows)
        counts[kind] = {'expected': expected, 'successful': valid, 'failed': expected - valid}
    successful = sum(c['successful'] for c in counts.values())
    summary = {'schema': 1, 'experiment': EXPERIMENT, 'scope': manifest['scope'], 'created_utc': utc_now(),
               'expected_trials': 66, 'successful_trials': successful, 'failed_trials': 66 - successful,
               'counts': counts, 'integrity_errors': integrity_errors,
               'prepare_sha256': preparation_hash, 'seed': manifest['seed'], 'groups': groups,
               'trials': rows, 'primary_contrasts': primary, 'service_allocation_contrasts': fairness,
               'profile_perturbation': perturbation, 'notes': manifest['notes']}
    figures = plots(out, rows)
    summary['figures'] = [{'file': name, 'sha256': sha(out / name)} for name in figures]
    save(out / 'summary.json', summary)
    flat = []
    for row in rows:
        for metric, value in row['metrics'].items():
            flat.append({'kind': row['kind'], 'case': row['case'], 'backend': row['backend'],
                         'repetition': row['repetition'], 'metric': metric, 'value': value})
    write_csv(out / 'per-trial-metrics.csv', flat)
    write_csv(out / 'failures.csv', [{'kind': r['kind'], 'case': r['case'], 'backend': r['backend'],
                                    'repetition': r['repetition'], 'raw_file': r['raw_file'], 'error': r['error']}
                                   for r in rows if r['error'] is not None])
    report = [EXPERIMENT, manifest['scope'], f'Successful {successful}/66; failures {66-successful}.',
              f'Primary {counts["primary"]}; separate diagnostics {counts["diagnostic"]}.',
              'All ranges are observed3-process ranges, not confidence intervals.', '']
    report.extend('INTEGRITY LIMITATION: ' + message for message in integrity_errors)
    report += ['COMPACT GROUPS (mean [min,max]; all trial values in summary.json/CSV)']
    for group in groups:
        fields = ['operations_per_s', 'synthetic_iteration_units_per_s',
                  'cpu_us_including_drain_per_ontime_op', 'response_p99_upper_us',
                  'cheap_mean_worker_operations_per_s', 'expensive_mean_worker_operations_per_s',
                  'service_jain', 'cheap_service_share', 'cheap_service_share_error']
        report.append(f'{group["kind"]} {group["case"]} {group["backend"]}: {group["successful"]}/3')
        for field in fields:
            value = group['metrics'].get(field)
            if value and value['n']:
                report.append(f'  {field}: {value["mean"]:.6g} [{value["min"]:.6g}, {value["max"]:.6g}]')
    report += ['', 'PREDECLARED PER-METRIC FC-PQ CONTRASTS (no aggregate winner)']
    for comparison in primary:
        report.append(f'{comparison["case"]} vs {comparison["comparator"]} {comparison["metric"]}: '
                      f'{comparison["classification"]}; favorable={comparison["favorable_direction"]}; '
                      f'ratios={[p["ratio"] for p in comparison["paired_points"]]}')
    report += ['', 'SEPARATE ELAPSED-SERVICE ALLOCATION; NOT OPERATION-COUNT FAIRNESS']
    for comparison in fairness:
        report.append(f'{comparison["case"]} FC-PQ vs {comparison["comparator"]}: '
                      f'{comparison["classification"]}; {comparison["paired_points"]}')
    report += ['', 'PROFILE PERTURBATION (instrumentation plus unequal window durations)']
    for comparison in perturbation:
        report.append(f'{comparison["case"]} {comparison["backend"]}: {comparison["paired_points"]}')
    report += ['', 'SCOPE, HYPOTHESES, AND LIMITATIONS', *manifest['notes']]
    (out / 'report.txt').write_text('\n'.join(report) + '\n')
    print(f'Analyzed saved data only: {out}; successful={successful}/66; integrity issues={len(integrity_errors)}')
    return bool(successful != 66 or integrity_errors)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--prepare-only', action='store_true')
    mode.add_argument('--run', action='store_true')
    mode.add_argument('--analyze-only', action='store_true')
    parser.add_argument('--output-root', type=Path, required=True)
    parser.add_argument('--binary-root', type=Path, default=FROZEN)
    parser.add_argument('--resource-registry', type=Path,
                        help='optional site-specific allocations.json; validate/copy CostBoundary slot')
    parser.add_argument('--analysis-root', type=Path)
    parser.add_argument('--seed', type=int, default=24092420, help='preparation order seed')
    args = parser.parse_args()
    args.output_root = args.output_root.resolve()
    args.binary_root = args.binary_root.resolve()
    if args.analysis_root:
        args.analysis_root = args.analysis_root.resolve()
    def stop(signum, frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, stop)
    if args.prepare_only:
        prepare(args)
        return 0
    if args.run:
        return int(run(args))
    return int(analyze(args))


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        sys.exit(f'Cost boundary error (existing evidence retained): {exc}')
