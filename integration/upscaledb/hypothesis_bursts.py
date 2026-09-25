#!/usr/bin/env python3
"""H3 exploratory co-run: prepare frozen wrappers, run 36 trials, analyze saved data.

Only preparation compiles (the C++ wrapper, never UpScaleDB or Rust). Preparation
and execution are separate so a coordinator can place concurrent explorations.
All artifact creation is exclusive; failed/incomplete roots are not resumable.
"""
import argparse
import csv
import hashlib
import itertools
import json
import math
import os
from pathlib import Path
import platform
import random
import re
import shlex
import shutil
import signal
import statistics
import subprocess
import sys
import time
from datetime import datetime, timezone

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
BACKENDS = ('native', 'fc', 'fc_pq', 'uscl')
PATTERNS = ('continuous', 'one_per_role', 'half_per_role')
CPUS = list(range(32, 40))
BURSTY = {'continuous': [], 'one_per_role': [0, 4], 'half_per_role': [0, 1, 4, 5]}
ORIGINAL_HARNESS = HERE / 'native_harness.cc'
EXPERIMENT = 'h3_intermittent_arrivals_exploration'
SCHEMA = 'h3-bursts-v1'
UNSET_ENV = ('NIX_CFLAGS_COMPILE', 'NIX_LDFLAGS', 'LD_PRELOAD', 'LD_LIBRARY_PATH', 'LD_AUDIT')


def utc():
    return datetime.now(timezone.utc).isoformat()


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def reject_constant(value):
    raise ValueError('nonfinite JSON number: ' + value)


def load(path):
    return json.loads(Path(path).read_text(), parse_constant=reject_constant)


def save(path, value):
    with Path(path).open('x') as target:
        json.dump(value, target, indent=2, sort_keys=True, allow_nan=False)
        target.write('\n')


def require(condition, message):
    if not condition:
        raise ValueError(message)


def frozen_add(files, path, expected=None):
    path = str(Path(path).absolute())
    actual = digest(path)
    require(expected is None or actual == expected, 'frozen hash mismatch: ' + path)
    require(path not in files or files[path] == actual, 'input changed: ' + path)
    files[path] = actual


def assert_frozen(files):
    for path, expected in files.items():
        require(digest(path) == expected, 'prepared input changed: ' + path)


def cpu_ranges(text):
    result = set()
    for piece in text.strip().split(','):
        if piece:
            bounds = piece.split('-')
            result.update(range(int(bounds[0]), int(bounds[-1]) + 1))
    return result


def machine():
    status = dict(line.split(':', 1) for line in Path('/proc/self/status').read_text().splitlines() if ':' in line)
    cpus = []
    for cpu in CPUS:
        base = Path('/sys/devices/system/cpu') / ('cpu' + str(cpu))
        cpus.append({'cpu': cpu,
                     'core': int((base / 'topology/core_id').read_text()),
                     'socket': int((base / 'topology/physical_package_id').read_text()),
                     'siblings': sorted(cpu_ranges((base / 'topology/thread_siblings_list').read_text())),
                     'nodes': sorted(int(p.name[4:]) for p in base.glob('node[0-9]*'))})
    return {'timestamp': utc(), 'hostname': platform.node(), 'uname': list(platform.uname()),
            'controller_affinity': sorted(os.sched_getaffinity(0)), 'cpus': cpus,
            'mems_allowed_list': status.get('Mems_allowed_list', '').strip(),
            'cpus_allowed_list': status.get('Cpus_allowed_list', '').strip(),
            'python': sys.version, 'python_executable': sys.executable}


def check_machine(receipt):
    require(set(CPUS) <= set(receipt['controller_affinity']), 'CPUs32-39 unavailable to controller')
    require(1 in cpu_ranges(receipt['mems_allowed_list']), 'NUMA node1 unavailable')
    require(all(c['nodes'] == [1] for c in receipt['cpus']), 'CPU placement is not node1')
    cores = {(c['socket'], c['core']) for c in receipt['cpus']}
    require(len(cores) == 8, 'CPUs32-39 must be eight distinct physical cores')


def clean_env():
    return {key: value for key, value in os.environ.items() if key not in UNSET_ENV}


def interrupt(signum, frame):
    raise InterruptedError('received signal ' + str(signum))


def invoke(directory, command, timeout, validator=None, files=None):
    """Always retain raw bytes and metadata, including spawn/timeout/validation failures."""
    directory.mkdir()
    metadata = {'schema': SCHEMA, 'command': command, 'cwd': str(ROOT),
                'started_at': utc(), 'timeout_seconds': timeout,
                'environment_unset': list(UNSET_ENV), 'success': False,
                'returncode': None, 'timed_out': False, 'result': None}
    process = None
    start = time.monotonic_ns()
    with (directory / 'stdout').open('xb') as stdout, (directory / 'stderr').open('xb') as stderr:
        try:
            if files is not None:
                assert_frozen(files)
            process = subprocess.Popen(command, cwd=ROOT, env=clean_env(), stdout=stdout,
                                       stderr=stderr, start_new_session=True)
            metadata['pid'] = process.pid
            metadata['returncode'] = process.wait(timeout=timeout)
            require(metadata['returncode'] == 0, 'nonzero process exit')
            if files is not None:
                assert_frozen(files)
            if validator is not None:
                stdout.flush()
                result = load(directory / 'stdout')
                metadata['result'] = result
                validator(result)
            metadata['success'] = True
        except (OSError, ValueError, KeyError, TypeError, subprocess.TimeoutExpired, KeyboardInterrupt) as exc:
            metadata['error'] = type(exc).__name__ + ': ' + str(exc)
            metadata['timed_out'] = isinstance(exc, subprocess.TimeoutExpired)
        finally:
            if process is not None:
                if process.poll() is None or metadata['timed_out']:
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                metadata['returncode'] = process.wait()
            metadata['finished_at'] = utc()
            metadata['wall_elapsed_ns'] = time.monotonic_ns() - start
    metadata['stdout_sha256'] = digest(directory / 'stdout')
    metadata['stderr_sha256'] = digest(directory / 'stderr')
    save(directory / 'record.json', metadata)
    require(metadata['success'], 'failed invocation retained at ' + str(directory))
    return metadata


def mapping(pattern):
    return [{'id': i, 'cpu': cpu, 'role': 'find' if i < 4 else 'insert',
             'group': 'bursty' if i in BURSTY[pattern] else 'always'} for i, cpu in enumerate(CPUS)]


def trial_command(taskset, binary, pattern, seconds, seed, canary=False):
    command = [taskset, '-c', '32-39', str(binary), '--pattern', pattern,
               '--mode', 'duration', '--finders', '4', '--inserters', '4',
               '--cpus', ','.join(map(str, CPUS)), '--init-cpu', '32', '--init-node', '1',
               '--memory-policy', 'bind', '--memory-nodes', '1', '--memory-limit-gib', '32',
               '--warmup', '0', '--preload', '100000', '--max-inserts', '200000000',
               '--seconds', str(seconds), '--seed', str(seed)]
    return command + ['--self-test', 'errors'] if canary else command


def validate(result, backend, pattern, seconds, seed, canary=False):
    if canary:
        require(result.get('self_test') == 'errors' and result.get('status') == 'ok', 'error canary failed')
        return
    require(result['schema'] == SCHEMA and result['experiment'] == EXPERIMENT, 'wrong wrapper schema')
    require(result['pattern'] == pattern and result['burst_operations'] == 64 and
            result['sleep_requested_ns'] == 5000000 and result['bin_width_ns'] == 100000000,
            'wrong burst configuration')
    require(result['inherited_affinity'] == CPUS and result['setup_affinity'] == [32], 'wrong receipt masks')
    base = result['base']
    expected = {'schema': 1, 'status': 'ok', 'variant': backend, 'mode': 'duration',
                'seed': seed, 'cpu_list': CPUS, 'setup_cpu': 32, 'init_node': 1,
                'finders': 4, 'inserters': 4, 'preload': 100000, 'max_inserts': 200000000,
                'memory_limit_gib': 32, 'warmup_seconds': 0, 'duration_requested_seconds': seconds}
    for key, value in expected.items():
        require(base[key] == value, 'base configuration mismatch: ' + key)
    require(base['deadline_offset_ns'] == round(seconds * 1e9), 'wrong deadline')
    numa = base['numa_memory']
    require(numa['requested'] == 'bind' and numa['effective'] == 'bind' and
            numa['nodes'] == [1] and numa['effective_nodes'] == [1], 'node1 bind not received')
    verification = base['verification']
    require(verification['seen'] == verification['db_count'] == verification['expected_count'] and
            all(verification[key] == 0 for key in ('bad_entries', 'integrity_status', 'cursor_status', 'count_status')),
            'database verification failed')
    require(base['shutdown'] == {'db_status': 0, 'env_status': 0}, 'database shutdown failed')
    workers = {w['id']: w for w in base['workers']}
    schedule = {w['id']: w for w in result['worker_schedule']}
    require(len(base['workers']) == len(result['worker_schedule']) == 8 and
            set(workers) == set(schedule) == set(range(8)), 'wrong worker set')
    for item in mapping(pattern):
        worker, receipt = workers[item['id']], schedule[item['id']]
        require(receipt['group'] == item['group'] and receipt['role'] == item['role'] and
                receipt['received_affinity'] == [item['cpu']], 'wrong worker placement or grouping')
        require(all(worker[key] == item['cpu'] for key in ('cpu_requested', 'cpu_start', 'cpu_end')) and
                worker['affinity_error'] == 0, 'base worker affinity mismatch')
        require(worker['finder'] == (item['role'] == 'find') and
                worker['inserter'] == (item['role'] == 'insert'), 'wrong dedicated role')
        require(len(receipt['completion_bins']) == math.ceil(seconds * 10) and
                all(isinstance(n, int) and n >= 0 for n in receipt['completion_bins']), 'invalid completion bins')
        intervals = receipt['sleep_intervals_ns']
        require(all(0 <= a <= b for a, b in intervals) and
                all(intervals[i - 1][1] <= intervals[i][0] for i in range(1, len(intervals))),
                'invalid sleep intervals')
        require(sum(b - a for a, b in intervals) == receipt['voluntary_sleep_ns'], 'sleep total mismatch')
        if item['group'] == 'always':
            require(not intervals and receipt['voluntary_sleep_ns'] == 0, 'always worker slept voluntarily')
        histogram = worker['latency_ns'][item['role']]
        require(len(histogram['bins']) == 64 and sum(histogram['bins']) == histogram['samples'],
                'invalid request histogram')


def prepare(args):
    root = args.output_root
    root.mkdir(parents=True, exist_ok=False)
    (root / 'bin').mkdir()
    (root / 'prepare').mkdir()
    receipt = machine()
    check_machine(receipt)
    files = {}
    for name in ('hypothesis_bursts.py', 'hypothesis_bursts.cc', 'native_harness.cc', 'bridge.h', 'private_ops.h'):
        frozen_add(files, HERE / name)
    taskset = shutil.which('taskset')
    ldd = shutil.which('ldd')
    require(taskset is not None and ldd is not None, 'taskset and ldd required for preparation')
    frozen_add(files, taskset)
    frozen_add(files, ldd)
    frozen_add(files, sys.executable)
    builds = {}
    for backend in BACKENDS:
        path = args.binary_root / ('build-' + backend + '.json')
        frozen_add(files, path)
        manifest = load(path)
        require(manifest['variant'] == backend, 'wrong archived backend')
        require(manifest['hashes']['harness_sha256'] == digest(ORIGINAL_HARNESS),
                'frozen build does not match this checkout native harness')
        frozen_add(files, HERE / 'bridge.h', manifest['hashes']['bridge_h_sha256'])
        frozen_add(files, HERE / 'private_ops.h', manifest['hashes']['private_ops_sha256'])
        frozen_add(files, manifest['binary'], manifest['hashes']['binary_sha256'])
        library = manifest['library_verification']
        frozen_add(files, library['shared_object'], library['shared_object_sha256'])
        archive = manifest['rust_staticlib']
        if archive:
            frozen_add(files, archive['archive'], archive['archive_sha256'])
        command = list(manifest['build']['harness_command'])
        frozen_add(files, command[0], manifest['toolchain']['cxx_sha256'])
        source = str(HERE / 'native_harness.cc')
        require(command.count(source) == 1 and command.count('-o') == 1, 'unexpected archived compile command')
        binary = root / 'bin' / ('hypothesis-bursts-' + backend)
        require(command[command.index('-o') + 1] == manifest['binary'], 'unexpected archived output')
        command[command.index(source)] = str(HERE / 'hypothesis_bursts.cc')
        command[command.index('-o') + 1] = str(binary)
        builds[backend] = {'archived_manifest': manifest, 'manifest_path': str(path),
                           'adapted_command': command, 'cwd': str(ROOT), 'binary': str(binary)}
    rng = random.Random(args.shuffle_seed)
    schedule = []
    for rep in range(1, 4):
        seed = rng.randrange(1, 2**63)
        cases = list(itertools.product(args.patterns, BACKENDS))
        rng.shuffle(cases)
        for pattern, backend in cases:
            trial_id = 'trial-%02d' % (len(schedule) + 1)
            schedule.append({'id': trial_id, 'rep': rep, 'seed': seed, 'pattern': pattern, 'backend': backend,
                             'command': trial_command(taskset, builds[backend]['binary'], pattern, 4, seed)})
    manifest = {'schema': SCHEMA, 'experiment': EXPERIMENT, 'classification': 'exploratory co-run',
                'created_at': utc(), 'co_run_label': args.co_run_label, 'shuffle_seed': args.shuffle_seed,
                'binary_root': str(args.binary_root), 'output_root': str(root), 'timeout_seconds': args.timeout_seconds,
                'repetitions': 3, 'seconds': 4, 'smoke_seconds': 0.2, 'smoke_invocations': 16,
                'patterns': list(args.patterns), 'expected_trials': len(schedule),
                'configuration': {'cpus': CPUS, 'init_cpu': 32, 'numa_node': 1, 'memory_policy': 'bind',
                                  'memory_limit_gib': 32, 'finders': 4, 'inserters': 4, 'warmup_seconds': 0,
                                  'preload': 100000, 'max_inserts': 200000000, 'burst_operations': 64,
                                  'sleep_requested_ns': 5000000, 'bin_width_ns': 100000000},
                'mapping': {p: mapping(p) for p in args.patterns}, 'machine': receipt,
                'environment_unset': list(UNSET_ENV), 'builds': builds, 'schedule': schedule,
                'initial_files': dict(files),
                'interpretation': 'n=3 observed ranges, not confidence intervals; no count JFI; voluntary sleep is not request latency or starvation; no idle-backlog inference'}
    cohort = root.parent / 'cohort.json'
    if cohort.is_file():
        manifest['cohort'] = {'path': str(cohort), 'sha256': digest(cohort), 'record': load(cohort)}
    save(root / 'manifest.json', manifest)
    frozen_add(files, root / 'manifest.json')
    for backend, build in builds.items():
        command = build['adapted_command']
        invoke(root / 'prepare' / (backend + '-compiler'), [command[0], '--version'], args.timeout_seconds)
        invoke(root / 'prepare' / (backend + '-compile'), command, args.timeout_seconds, files=files)
        frozen_add(files, build['binary'])
        # -M emits the full preprocessor dependency closure, including system headers;
        # the actual wrapper compile above changes only source and output operands.
        dependency_file = root / 'prepare' / (backend + '-dependencies.d')
        dependency_command = list(command)
        dependency_command[dependency_command.index('-o') + 1] = str(dependency_file)
        dependency_command.append('-M')
        invoke(root / 'prepare' / (backend + '-dependencies'), dependency_command, args.timeout_seconds, files=files)
        dependency_text = dependency_file.read_text().replace('\\\n', ' ')
        require(': ' in dependency_text, 'unrecognized compiler dependency output')
        headers = shlex.split(dependency_text.split(': ', 1)[1])
        require(headers, 'empty compiler dependency closure')
        for path in headers:
            frozen_add(files, Path(path) if Path(path).is_absolute() else ROOT / path)
        ldd_dir = root / 'prepare' / (backend + '-ldd')
        invoke(ldd_dir, [ldd, build['binary']], args.timeout_seconds, files=files)
        text = (ldd_dir / 'stdout').read_text()
        require('not found' not in text, 'unresolved runtime dependency')
        libraries = []
        for line in text.splitlines():
            match = re.search(r'(?:=>\s+)?(/[^\s]+)\s+\(', line)
            if match:
                libraries.append(match.group(1))
                frozen_add(files, match.group(1))
        require(libraries, 'ldd did not resolve runtime dependencies')
        save(root / 'prepare' / (backend + '-runtime-libraries.json'), libraries)
        for pattern in PATTERNS:
            command = trial_command(taskset, build['binary'], pattern, 0.2, 1)
            invoke(root / 'prepare' / (backend + '-' + pattern), command, args.timeout_seconds,
                   lambda result, b=backend, p=pattern: validate(result, b, p, 0.2, 1), files)
        command = trial_command(taskset, build['binary'], 'continuous', 0.2, 1, True)
        invoke(root / 'prepare' / (backend + '-errors'), command, args.timeout_seconds,
               lambda result, b=backend: validate(result, b, 'continuous', 0.2, 1, True), files)
    assert_frozen(files)
    save(root / 'prepared.json', {'schema': SCHEMA, 'status': 'prepared', 'completed_at': utc(),
                                 'smoke_invocations': 16, 'files': files})
    return manifest


def run(root):
    prepared = load(root / 'prepared.json')
    require(prepared['schema'] == SCHEMA and prepared['status'] == 'prepared' and
            prepared['smoke_invocations'] == 16, 'root is not prepared')
    files = prepared['files']
    assert_frozen(files)
    manifest = load(root / 'manifest.json')
    require(manifest['output_root'] == str(root), 'prepared root cannot be relocated')
    receipt = machine()
    check_machine(receipt)
    require(receipt['cpus'] == manifest['machine']['cpus'], 'topology changed after preparation')
    directory = root / 'run'
    directory.mkdir()
    save(directory / 'manifest.json', {'schema': SCHEMA, 'started_at': utc(), 'machine': receipt,
                                      'prepared_sha256': digest(root / 'prepared.json'),
                                      'manifest_sha256': digest(root / 'manifest.json'),
                                      'co_run_label': manifest['co_run_label'], 'schedule': manifest['schedule']})
    status = {'schema': SCHEMA, 'started_at': utc(), 'status': 'failed', 'completed': []}
    try:
        for item in manifest['schedule']:
            invoke(directory / item['id'], item['command'], manifest['timeout_seconds'],
                   lambda result, item=item: validate(result, item['backend'], item['pattern'], 4, item['seed']), files)
            status['completed'].append(item['id'])
        status['status'] = 'ok'
    except (OSError, ValueError, KeyError, TypeError, KeyboardInterrupt) as exc:
        status['error'] = str(exc)
        raise
    finally:
        status['finished_at'] = utc()
        save(directory / 'status.json', status)


def p99_bound(bins):
    samples = sum(bins)
    if samples < 10000:
        return None
    rank, cumulative = (samples * 99 + 99) // 100, 0
    for index, count in enumerate(bins):
        cumulative += count
        if cumulative >= rank:
            return None if index == 63 else (0 if index == 0 else 1 << index)
    return None


def rows_for(item, result):
    workers = {worker['id']: worker for worker in result['base']['workers']}
    rows = []
    for receipt in result['worker_schedule']:
        worker = workers[receipt['id']]
        role = receipt['role']
        index = 0 if role == 'find' else 1
        hist = worker['latency_ns'][role]
        rows.append({'trial': item['id'], 'rep': item['rep'], 'backend': item['backend'], 'pattern': item['pattern'],
                     'worker': receipt['id'], 'role': role, 'group': receipt['group'],
                     'completed_before_deadline': worker['completed_before_deadline'][index],
                     'completed_including_drain': worker['completed'][index],
                     'throughput_ops_s': worker['completed_before_deadline'][index] / 4,
                     'request_samples': hist['samples'],
                     'request_p99_upper_bound_ns': hist['quantile_upper_bound_ns']['p99'],
                     'request_max_ns': hist['max_ns'], 'voluntary_sleep_ns': receipt['voluntary_sleep_ns'],
                     'longest_completion_gap_including_sleep_ns': receipt['longest_completion_gap_including_sleep_ns'],
                     'bins': hist['bins'], 'completion_bins': receipt['completion_bins'],
                     'sleep_intervals_ns': receipt['sleep_intervals_ns']})
    return rows


def group_rows(workers):
    rows = []
    keys = sorted({(w['trial'], w['role'], w['group']) for w in workers})
    for trial, role, group in keys:
        members = [w for w in workers if (w['trial'], w['role'], w['group']) == (trial, role, group)]
        first = members[0]
        bins = [sum(w['bins'][i] for w in members) for i in range(64)]
        maxima = [w['request_max_ns'] for w in members if w['request_max_ns'] is not None]
        rows.append({key: first[key] for key in ('trial', 'rep', 'backend', 'pattern', 'role', 'group')} |
                    {'workers': len(members), 'throughput_ops_s': sum(w['throughput_ops_s'] for w in members),
                     'throughput_per_worker_ops_s': sum(w['throughput_ops_s'] for w in members) / len(members),
                     'request_samples': sum(bins), 'request_p99_upper_bound_ns': p99_bound(bins),
                     'request_max_ns': max(maxima) if maxima else None,
                     'voluntary_sleep_ns': sum(w['voluntary_sleep_ns'] for w in members),
                     'longest_completion_gap_including_sleep_ns': max(w['longest_completion_gap_including_sleep_ns'] for w in members)})
    return rows


def write_csv(path, rows, columns):
    with path.open('x', newline='') as target:
        writer = csv.DictWriter(target, fieldnames=columns, extrasaction='ignore')
        writer.writeheader()
        writer.writerows(rows)


def figures(directory, groups, label, failure_count, missing_count, patterns):
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    metrics = [('throughput_per_worker_ops_s', 'Group throughput per worker (ops/s)', 1),
               ('request_p99_upper_bound_ns', 'Pooled request p99 upper bound (ms)', 1e-6),
               ('request_max_ns', 'Maximum request response (ms)', 1e-6)]
    series = list(itertools.product(('find', 'insert'), ('always', 'bursty')))
    for metric, ylabel, scale in metrics:
        fig, axes = plt.subplots(1, len(patterns), figsize=(6 * len(patterns), 5), squeeze=False)
        for ax, pattern in zip(axes[0], patterns):
            for offset, (role, group) in enumerate(series):
                color = 'C' + str(offset)
                for backend_index, backend in enumerate(BACKENDS):
                    values = [r[metric] * scale for r in groups if r['pattern'] == pattern and r['backend'] == backend and
                              r['role'] == role and r['group'] == group and r[metric] is not None]
                    x = backend_index + (offset - 1.5) * 0.17
                    if values:
                        ax.plot([x, x], [min(values), max(values)], color=color, linewidth=1)
                        ax.scatter([x + (i - (len(values) - 1) / 2) * 0.025 for i in range(len(values))],
                                   values, color=color, s=20)
            for offset, (role, group) in enumerate(series):
                ax.scatter([], [], color='C' + str(offset), label=role + '/' + group)
            ax.set_xticks(range(4), BACKENDS)
            ax.set_title(pattern)
            ax.set_ylabel(ylabel)
            ax.grid(axis='y', alpha=0.25)
        axes[0][-1].legend(fontsize=8)
        fig.suptitle('H3 exploratory co-run: ' + label + '\nRaw trial points; observed min–max, not CIs; failures=%d, missing=%d' % (failure_count, missing_count))
        fig.tight_layout()
        fig.savefig(directory / (metric + '.png'), dpi=150)
        plt.close(fig)
    return {'matplotlib_version': matplotlib.__version__, 'metrics': [m[0] for m in metrics]}


def analyze(root):
    manifest = load(root / 'manifest.json')
    directory = root / 'analysis'
    directory.mkdir()
    trials, workers = [], []
    for item in manifest['schedule']:
        path = root / 'run' / item['id'] / 'record.json'
        trial = {'item': item, 'status': 'missing', 'record_path': str(path)}
        if path.exists():
            try:
                record = load(path)
                trial['record'] = record
                require(record['success'], record.get('error', 'recorded process failure'))
                validate(record['result'], item['backend'], item['pattern'], 4, item['seed'])
                trial['status'] = 'ok'
                workers.extend(rows_for(item, record['result']))
            except (OSError, ValueError, KeyError, TypeError) as exc:
                trial['status'] = 'failed'
                trial['error'] = str(exc)
        elif path.parent.exists():
            trial['status'] = 'failed'
            trial['error'] = 'invocation started but metadata missing; raw files may survive interruption'
        trials.append(trial)
    groups = group_rows(workers)
    failure_count = sum(t['status'] == 'failed' for t in trials)
    missing_count = sum(t['status'] == 'missing' for t in trials)
    summaries = []
    metrics = ('throughput_ops_s', 'throughput_per_worker_ops_s', 'request_p99_upper_bound_ns',
               'request_max_ns', 'voluntary_sleep_ns', 'longest_completion_gap_including_sleep_ns')
    for pattern, backend, role, group in itertools.product(manifest['patterns'], BACKENDS, ('find', 'insert'), ('always', 'bursty')):
        if pattern == 'continuous' and group == 'bursty':
            continue
        selected = [row for row in groups if (row['pattern'], row['backend'], row['role'], row['group']) == (pattern, backend, role, group)]
        case_trials = [t for t in trials if (t['item']['pattern'], t['item']['backend']) == (pattern, backend)]
        entry = {'pattern': pattern, 'backend': backend, 'role': role, 'group': group,
                 'expected_n': 3, 'successful_n': len(selected),
                 'failed_n': sum(t['status'] == 'failed' for t in case_trials),
                 'missing_n': sum(t['status'] == 'missing' for t in case_trials), 'metrics': {}}
        for metric in metrics:
            points = [row[metric] for row in selected if row[metric] is not None]
            entry['metrics'][metric] = {'points': points, 'available_n': len(points),
                                        'median': statistics.median(points) if points else None,
                                        'observed_min': min(points) if points else None,
                                        'observed_max': max(points) if points else None}
        summaries.append(entry)
    summary = {'schema': SCHEMA, 'experiment': EXPERIMENT, 'classification': 'exploratory co-run',
               'co_run_label': manifest['co_run_label'], 'analyzed_at': utc(),
               'manifest_sha256': digest(root / 'manifest.json'),
               'complete': failure_count == missing_count == 0 and len(trials) == manifest['expected_trials'],
               'expected_trials': manifest['expected_trials'], 'successful_trials': len(trials) - failure_count - missing_count,
               'failed_trials': failure_count, 'missing_trials': missing_count,
               'interpretation': manifest['interpretation'], 'trials': trials, 'workers': workers,
               'groups': groups, 'summaries': summaries}
    save(directory / 'summary.json', summary)
    worker_columns = ['trial', 'rep', 'backend', 'pattern', 'worker', 'role', 'group', 'completed_before_deadline',
                      'completed_including_drain', 'throughput_ops_s', 'request_samples', 'request_p99_upper_bound_ns',
                      'request_max_ns', 'voluntary_sleep_ns', 'longest_completion_gap_including_sleep_ns']
    group_columns = ['trial', 'rep', 'backend', 'pattern', 'role', 'group', 'workers', *metrics, 'request_samples']
    write_csv(directory / 'workers.csv', workers, worker_columns)
    write_csv(directory / 'groups.csv', groups, group_columns)
    write_csv(directory / 'trials.csv', [{'trial': t['item']['id'], 'backend': t['item']['backend'],
              'pattern': t['item']['pattern'], 'rep': t['item']['rep'], 'status': t['status'],
              'error': t.get('error', '')} for t in trials], ['trial', 'rep', 'backend', 'pattern', 'status', 'error'])
    lines = ['H3 intermittent arrivals: exploratory co-run (' + manifest['co_run_label'] + ')',
             'Status: %d/%d successful; %d failed; %d missing.' % (summary['successful_trials'], summary['expected_trials'], failure_count, missing_count),
             'Three repetitions per case; medians and observed min-max are descriptive, not confidence intervals.',
             'Successful measurements alone are plotted; every failed/missing trial remains in summary.json and trials.csv.',
             '4s denominator, 4 find + 4 insert workers, CPU32-39/node1 bind, 32GiB RLIMIT_AS, no warmup.',
             'Request response excludes voluntary sleep and includes drained requests; throughput counts on-time completions.',
             'Group p99 pools log2 histograms (upper bound); <10000 samples or overflow => unavailable.',
             'Actual voluntary sleep includes oversleep; completion gaps include that sleep and are not starvation evidence.',
             'No count-based JFI and no idle-backlog inference. Group sizes differ; plots normalize throughput per worker.',
             'CPU/process, NUMA, verification, placement, 100ms completion bins and actual sleep intervals remain in JSON.', '']
    for item in summaries:
        throughput = item['metrics']['throughput_per_worker_ops_s']
        delay = item['metrics']['request_max_ns']
        lines.append('%s %s %s/%s: n=%d/3, failed=%d missing=%d; ops/s/worker median=%s range=[%s,%s]; request max ns range=[%s,%s]' %
                     (item['pattern'], item['backend'], item['role'], item['group'], item['successful_n'], item['failed_n'],
                      item['missing_n'], throughput['median'], throughput['observed_min'], throughput['observed_max'],
                      delay['observed_min'], delay['observed_max']))
    with (directory / 'report.txt').open('x') as target:
        target.write('\n'.join(lines) + '\n')
    plot_provenance = figures(directory, groups, manifest['co_run_label'], failure_count, missing_count, manifest['patterns'])
    save(directory / 'figures.json', plot_provenance)
    return directory / 'summary.json'


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-root', type=Path, required=True, help='fresh H3 subdirectory, e.g. .worktree/upscaledb-hypotheses/h3')
    parser.add_argument('--binary-root', type=Path, default=ROOT / '.worktree/upscaledb-joined-build')
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--prepare-only', action='store_true')
    mode.add_argument('--run', action='store_true', help='timed trials only; requires prepared root')
    mode.add_argument('--analyze-only', action='store_true', help='saved data only; requires absent analysis directory')
    parser.add_argument('--co-run-label', default='exploratory-co-run')
    parser.add_argument('--shuffle-seed', type=int, default=20260924)
    parser.add_argument('--patterns', '--scenarios', help='comma-list: all three patterns (default), or continuous plus one burst pattern; frozen at preparation')
    parser.add_argument('--timeout-seconds', type=float, default=120)
    args = parser.parse_args(argv)
    args.output_root = args.output_root.expanduser().resolve()
    args.binary_root = args.binary_root.expanduser().resolve()
    require(math.isfinite(args.timeout_seconds) and args.timeout_seconds > 0, 'timeout must be positive and finite')
    requested_patterns = args.patterns.split(',') if args.patterns is not None else None
    if requested_patterns is not None:
        require(len(set(requested_patterns)) == len(requested_patterns) and
                set(requested_patterns) <= set(PATTERNS) and
                'continuous' in requested_patterns and len(requested_patterns) in (2, 3),
                '--patterns requires continuous and one or both burst patterns')
    if args.run or args.analyze_only:
        saved_patterns = load(args.output_root / 'manifest.json')['patterns']
        require(requested_patterns is None or set(requested_patterns) == set(saved_patterns),
                '--patterns differs from prepared schedule; use a fresh output root')
        args.patterns = saved_patterns
    else:
        args.patterns = [pattern for pattern in PATTERNS if requested_patterns is None or pattern in requested_patterns]
    signal.signal(signal.SIGTERM, interrupt)
    signal.signal(signal.SIGINT, interrupt)
    if args.analyze_only:
        print(analyze(args.output_root))
    elif args.run:
        run(args.output_root)
        print(args.output_root / 'run/status.json')
    else:
        prepare(args)
        if not args.prepare_only:
            run(args.output_root)
        print(args.output_root / ('prepared.json' if args.prepare_only else 'run/status.json'))
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError, TypeError) as exc:
        sys.exit(str(exc))
