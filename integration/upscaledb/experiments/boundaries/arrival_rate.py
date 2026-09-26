#!/usr/bin/env python3
"""SYNTHETIC Poisson arrivals on actual UpScaleDB, not YCSB or trace replay.

Preparation compiles only the companion against frozen joined-build libraries,
checks correctness, runs six native closed-loop capacity calibrations, and freezes
all 90 comparative commands and 18 shared schedules. Main must hold the exclusive
global measurement lock plus arrival slot lock for BOTH prepare and run. This
controller inherits those locks; it must not acquire nested flock locks.
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
import struct
import subprocess
import sys
import tarfile
import time
from datetime import datetime, timezone

from integration.upscaledb._paths import CORE, ROOT, UPSCALEDB

HERE = Path(__file__).resolve().parent
FROZEN = ROOT / '.worktree/upscaledb'
BACKENDS = ('native', 'bridge_mutex', 'fc', 'fc_pq', 'uscl')
MIXES = (95, 50)
FACTORS = (0.3, 0.7, 1.1)
CPUS = list(range(40, 48))
REPETITIONS = 3
WINDOW_NS = 2_000_000_000
CAP = 2_000_000
SCHEMA = 'boundary-arrival-v1'
EXPERIMENT = 'synthetic_poisson_upscaledb'
ORIGINAL_HARNESS = CORE / 'native_harness.cc'
UNSET = ('NIX_CFLAGS_COMPILE', 'NIX_LDFLAGS', 'LD_PRELOAD', 'LD_LIBRARY_PATH', 'LD_AUDIT')
SCOPE = ('Synthetic independent per-worker Poisson arrivals to a real restricted in-memory UpScaleDB; '
         'eight stable requesters; immutable preloaded reads and collision-free inserts; no YCSB, '
         'production trace, fairness profiling, or idle-with-pending-work claim. Native calibration is '
         'closed-loop completed capacity, not exact open-loop saturation. Factors are relative to the '
         'same frozen native baseline, not backend-specific utilization. n=3 exploratory ranges, no CIs.')


def utc():
    return datetime.now(timezone.utc).isoformat()


def require(ok, message):
    if not ok:
        raise ValueError(message)


def load(path):
    def bad(value):
        raise ValueError('nonfinite JSON: ' + value)
    return json.loads(Path(path).read_text(), parse_constant=bad)


def save(path, value):
    with Path(path).open('x') as target:
        json.dump(value, target, indent=2, sort_keys=True, allow_nan=False)
        target.write('\n')


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def freeze(files, path, expected=None):
    path = str(Path(path).absolute())
    actual = digest(path)
    require(expected is None or actual == expected, 'frozen hash mismatch: ' + path)
    require(path not in files or files[path] == actual, 'input changed during preparation: ' + path)
    files[path] = actual


def assert_frozen(files):
    for path, expected in files.items():
        require(digest(path) == expected, 'prepared input changed: ' + path)


def cpu_ranges(text):
    result = set()
    for item in text.strip().split(','):
        if item:
            ends = item.split('-')
            result.update(range(int(ends[0]), int(ends[-1]) + 1))
    return result


def machine():
    status = dict(line.split(':', 1) for line in Path('/proc/self/status').read_text().splitlines() if ':' in line)
    cpus = []
    for cpu in CPUS:
        base = Path('/sys/devices/system/cpu') / f'cpu{cpu}'
        cpus.append({'cpu': cpu, 'core': int((base / 'topology/core_id').read_text()),
                     'socket': int((base / 'topology/physical_package_id').read_text()),
                     'siblings': sorted(cpu_ranges((base / 'topology/thread_siblings_list').read_text())),
                     'nodes': sorted(int(p.name[4:]) for p in base.glob('node[0-9]*'))})
    receipt = {'timestamp': utc(), 'hostname': platform.node(), 'uname': list(platform.uname()),
               'affinity': sorted(os.sched_getaffinity(0)), 'cpus': cpus,
               'mems_allowed_list': status.get('Mems_allowed_list', '').strip(),
               'python': sys.version, 'python_executable': sys.executable,
               'numa_balancing': Path('/proc/sys/kernel/numa_balancing').read_text().strip()}
    require(receipt['affinity'] == CPUS, 'controller must inherit exactly CPUs40-47, without SMT siblings')
    require(1 in cpu_ranges(receipt['mems_allowed_list']), 'node1 memory unavailable')
    require(all(c['nodes'] == [1] for c in cpus), 'requested CPUs must be node1')
    require(len({(c['socket'], c['core']) for c in cpus}) == 8, 'eight distinct physical cores required')
    return receipt


def invoke(directory, command, timeout, validator=None):
    """Retain even nonzero exits, parsed incomplete records and killed-process bytes."""
    directory.mkdir()
    record = {'command': command, 'cwd': str(ROOT), 'started_at': utc(), 'timeout_seconds': timeout,
              'success': False, 'returncode': None, 'timed_out': False, 'result': None,
              'environment_unset': list(UNSET), 'inherited_affinity': sorted(os.sched_getaffinity(0))}
    process = None
    begin = time.monotonic_ns()
    with (directory / 'stdout').open('xb') as stdout, (directory / 'stderr').open('xb') as stderr:
        try:
            process = subprocess.Popen(command, cwd=ROOT,
                                       env={k: v for k, v in os.environ.items() if k not in UNSET},
                                       stdout=stdout, stderr=stderr, start_new_session=True)
            record['pid'] = process.pid
            record['returncode'] = process.wait(timeout=timeout)
        except (OSError, subprocess.TimeoutExpired, KeyboardInterrupt) as exc:
            record['error'] = type(exc).__name__ + ': ' + str(exc)
            record['timed_out'] = isinstance(exc, subprocess.TimeoutExpired)
        finally:
            if process is not None:
                if process.poll() is None:
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                record['returncode'] = process.wait()
    if validator is not None:
        try:
            record['result'] = load(directory / 'stdout')
            validator(record['result'])
        except (ValueError, KeyError, TypeError) as exc:
            record['validation_error'] = type(exc).__name__ + ': ' + str(exc)
    record['success'] = record['returncode'] == 0 and 'error' not in record and 'validation_error' not in record
    record['finished_at'] = utc()
    record['wall_ns'] = time.monotonic_ns() - begin
    record['stdout_sha256'] = digest(directory / 'stdout')
    record['stderr_sha256'] = digest(directory / 'stderr')
    save(directory / 'record.json', record)
    return record


def checked(*args, **kwargs):
    record = invoke(*args, **kwargs)
    require(record['success'], 'preparation failed; evidence retained at ' + str(args[0]))
    return record


def command(binary, mix, seed, schedule=None, window_ns=WINDOW_NS, calibration=False):
    result = [str(binary), '--find-percent', str(mix), '--seed', str(seed),
              '--seconds', str(window_ns / 1e9), '--arrival-mode', 'calibration' if calibration else 'open']
    if schedule is not None:
        result += ['--schedule', str(schedule)]
    return result


def validate(result, mix=None, seed=None, generated=None, calibration=False, window_ns=WINDOW_NS):
    require(result['schema'] == SCHEMA, 'wrong result schema')
    require(result['status'] in ('ok', 'incomplete'), 'failed or hard-timeout outcome')
    require(result['find_percent'] == mix and result['seed'] == seed, 'wrong mix/seed')
    require(result['window_ns'] == window_ns, 'wrong window')
    require(result['mode'] == ('closed_loop_calibration' if calibration else 'synthetic_open_loop'), 'wrong mode')
    require(result['memory_policy'] == 'bind' and result['memory_nodes'] == [1], 'wrong memory placement')
    require(result['inherited_affinity'] == CPUS, 'wrong process affinity')
    require(result['inflight_final'] == 0, 'joined workers left an inflight call')
    require(result['submitted'] == result['returned'] == result['successful'], 'error or missing returned operation')
    require(result['failed_returns'] == 0, 'failed returned operation')
    require(result['submitted_before_deadline'] >= result['returned_before_deadline'] == result['successful_before_deadline'],
            'invalid deadline accounting')
    require(result['successful_during_drain'] == result['successful'] - result['successful_before_deadline'], 'bad drain count')
    v = result['verification']
    require(v['bad_entries'] == v['integrity_status'] == v['count_status'] == v['cursor_status'] == 0, 'oracle error')
    require(v['seen'] == v['db_count'] == v['expected_count'], 'exact set oracle failed')
    require(result['shutdown'] == [0, 0], 'exclusive shutdown failed')
    require(len(result['workers']) == 8, 'wrong worker count')
    for worker, cpu in zip(result['workers'], CPUS):
        require(worker['id'] == cpu - 40 and worker['received_affinity'] == [cpu] and
                worker['cpu_start'] == worker['cpu_end'] == cpu and worker['affinity_error'] == 0, 'worker affinity mismatch')
        require(not worker['error'] and worker['first_db_status'] == worker['bridge_status'] ==
                worker['bad_value'] == worker['bad_size'] == 0, 'worker operation error')
        for metric in ('scheduled_to_return_ns', 'submit_to_return_ns', 'submit_lag_ns'):
            hists = list(itertools.chain.from_iterable(worker[metric].values()))
            require(sum(h['samples'] for h in hists) == worker['returned'], 'missing latency sample')
            require(all(len(h['bins']) == 64 and sum(h['bins']) == h['samples'] for h in hists), 'bad histogram')
        if not calibration:
            require(worker['unreturned_censor_age_ns']['samples'] + worker['returned'] == worker['generated'],
                    'coordinated omission in worker outcome accounting')
    for metric in ('submitted', 'returned', 'successful', 'submitted_before_deadline',
                   'returned_before_deadline', 'successful_before_deadline'):
        require(sum(w[metric] for w in result['workers']) == result[metric], 'inconsistent total ' + metric)
    require(v['expected_count'] == 100000 + sum(w['insert_prefix'] for w in result['workers']), 'insert-prefix oracle mismatch')
    if not calibration:
        require(result['generated'] == generated == sum(w['generated'] for w in result['workers']), 'wrong offered count')
        require(result['unsubmitted_at_deadline'] + result['inflight_at_deadline'] == result['backlog_at_deadline'], 'bad backlog split')
        require(result['backlog_at_deadline'] + result['returned_before_deadline'] == generated, 'missing arrival-window outcomes')
        require(result['unsubmitted_final'] + result['returned'] == generated, 'missing final outcomes')
        require(result['unsatisfied_final'] == generated - result['successful'], 'wrong success denominator')
        require((result['status'] == 'ok') == (result['unsatisfied_final'] == 0), 'wrong completion status')
    else:
        require(result['status'] == 'ok' and result['successful_before_deadline'] > 0, 'invalid native calibration')


def schedule_file(path, seed, mix, rate, window_ns=WINDOW_NS):
    """Streaming generation, <=16MB successful schedule, never silently truncate.

    Eight independent rate/8 Poisson streams; operation choices/key inputs are a
    separate deterministic function of (seed, requester, index) in the harness.
    A cap hit retains the rejected candidate plus explicit reason, then prepare
    lowers the common baseline and regenerates ALL rates for that mix.
    """
    counts = []
    total = 0
    with path.open('xb') as target:
        target.write(b'ARRIVAL1' + struct.pack('<11Q', seed, window_ns, mix, *([0] * 8)))
        for worker in range(8):
            rng = random.Random((seed << 8) + worker)
            arrival = 0.0
            count = 0
            while True:
                arrival += rng.expovariate(rate / 8)
                if arrival >= window_ns / 1e9:
                    break
                if total == CAP:
                    return {'accepted': False, 'generated_prefix': total, 'cap': CAP, 'reason': 'next arrival exceeds cap'}
                target.write(struct.pack('<Q', min(window_ns - 1, int(arrival * 1e9))))
                count += 1
                total += 1
            counts.append(count)
        target.seek(32)
        target.write(struct.pack('<8Q', *counts))
    return {'accepted': True, 'generated': total, 'worker_counts': counts,
            'rate_per_second': rate, 'realized_offered_rate': total / (window_ns / 1e9),
            'window_ns': window_ns, 'seed': seed, 'find_percent': mix,
            'path': str(path), 'sha256': digest(path), 'bytes': path.stat().st_size}


def build_companions(args, files):
    builds = {}
    archive_sources = {HERE / 'arrival_rate.py', HERE / 'arrival_rate.cc',
                       ORIGINAL_HARNESS, CORE / 'bridge.h', CORE / 'private_ops.h',
                       UPSCALEDB / '_paths.py'}
    for source in archive_sources:
        freeze(files, source)
    ldd = shutil.which('ldd')
    require(ldd is not None, 'ldd required')
    freeze(files, ldd)
    freeze(files, sys.executable)
    for backend in BACKENDS:
        build_path = args.binary_root / f'build-{backend}.json'
        build = load(build_path)
        freeze(files, build_path)
        require(build['variant'] == backend and
                build['hashes']['harness_sha256'] == digest(ORIGINAL_HARNESS),
                'frozen build does not match this checkout native harness')
        for name, field in (('bridge.h', 'bridge_h_sha256'), ('private_ops.h', 'private_ops_sha256')):
            freeze(files, CORE / name, build['hashes'][field])
        library = build['library_verification']
        freeze(files, library['shared_object'], library['shared_object_sha256'])
        freeze(files, build['binary'], build['hashes']['binary_sha256'])
        if build['rust_staticlib']:
            rust = build['rust_staticlib']
            freeze(files, rust['archive'], rust['archive_sha256'])
        cmd = list(build['build']['harness_command'])
        freeze(files, cmd[0], build['toolchain']['cxx_sha256'])
        candidates = [i for i, token in enumerate(cmd) if Path(token).name == 'native_harness.cc']
        require(len(candidates) == 1 and cmd.count('-o') == 1, 'unsupported frozen compiler command')
        original_source = Path(cmd[candidates[0]])
        freeze(files, original_source, digest(ORIGINAL_HARNESS))
        require(cmd[cmd.index('-o') + 1] == build['binary'], 'unexpected frozen output')
        binary = args.output_root / 'bin' / f'boundary-arrival-{backend}'
        cmd[candidates[0]] = str(HERE / 'arrival_rate.cc')
        cmd[cmd.index('-o') + 1] = str(binary)
        # The frozen harness command includes the core/ include path for
        # native_harness.cc, bridge.h and private_ops.h.
        checked(args.output_root / 'prepare' / f'{backend}-compile', cmd, 180)
        freeze(files, binary)
        depfile = args.output_root / 'prepare' / f'{backend}-dependencies.d'
        depcmd = list(cmd)
        depcmd[depcmd.index('-o') + 1] = str(depfile)
        depcmd.append('-M')
        checked(args.output_root / 'prepare' / f'{backend}-dependencies', depcmd, 120)
        dependency_text = depfile.read_text().replace('\\\n', ' ')
        require(': ' in dependency_text, 'unrecognized dependency output')
        for name in shlex.split(dependency_text.split(': ', 1)[1]):
            source = Path(name) if Path(name).is_absolute() else ROOT / name
            freeze(files, source)
            archive_sources.add(source)
        linked = checked(args.output_root / 'prepare' / f'{backend}-ldd', [ldd, str(binary)], 30)
        text = (args.output_root / 'prepare' / f'{backend}-ldd' / 'stdout').read_text()
        require('not found' not in text, 'unresolved shared library')
        libraries = re.findall(r'(?:=>\s+)?(/[^\s]+)\s+\(', text)
        require(libraries, 'no runtime library closure')
        for path in libraries:
            freeze(files, path)
        builds[backend] = {'binary': str(binary), 'binary_sha256': digest(binary), 'adapted_command': cmd,
                           'original_manifest_path': str(build_path), 'original_manifest': build,
                           'runtime_libraries': libraries, 'ldd_record': linked}
    archive = args.output_root / 'sources.tar.gz'
    with tarfile.open(archive, 'x:gz') as target:
        for path in sorted(archive_sources):
            target.add(path, arcname='sources/' + str(path.absolute()).lstrip('/'), recursive=False)
    freeze(files, archive)
    return builds


def prepare(args):
    root = args.output_root
    root.mkdir(parents=True, exist_ok=False)
    for name in ('prepare', 'bin', 'schedules', 'calibration'):
        (root / name).mkdir()
    receipt = machine()
    save(root / 'intent.json', {'experiment': EXPERIMENT, 'scope': SCOPE, 'created_at': utc(), 'machine': receipt,
                               'expected_trials': 90, 'mixes': MIXES, 'native_rate_factors': FACTORS,
                               'repetitions': REPETITIONS, 'window_ns': WINDOW_NS, 'arrival_cap': CAP,
                               'ordering_seed': args.ordering_seed, 'argv': sys.argv})
    files = {}
    builds = build_companions(args, files)
    # Operation canaries and both mixed arrival paths precede capacity calibration.
    for mix in MIXES:
        path = root / 'schedules' / f'smoke-{mix}.bin'
        smoke = schedule_file(path, 7000 + mix, mix, 1000, 200_000_000)
        require(smoke['accepted'], 'smoke cap exceeded')
        freeze(files, path)
        for backend in BACKENDS:
            cmd = command(builds[backend]['binary'], mix, smoke['seed'], path, 200_000_000)
            checked(root / 'prepare' / f'{backend}-smoke-{mix}', cmd, 120,
                    lambda result, m=mix, s=smoke: validate(result, m, s['seed'], s['generated'], window_ns=200_000_000))
    for backend in BACKENDS:
        def canary(result):
            require(result['status'] == 'ok' and result['self_test'] == 'errors', 'canary failed')
        checked(root / 'prepare' / f'{backend}-canary', [builds[backend]['binary'], '--self-test', 'errors'], 120, canary)
    calibrations = {}
    schedules = []
    for mix in MIXES:
        trials = []
        for rep in range(REPETITIONS):
            seed = args.ordering_seed + mix * 100 + rep
            cmd = command(builds['native']['binary'], mix, seed, calibration=True)
            record = checked(root / 'calibration' / f'mix-{mix}-rep-{rep}', cmd, 120,
                             lambda result, m=mix, s=seed: validate(result, m, s, calibration=True))
            trials.append({'rep': rep, 'seed': seed, 'completed_rate': record['result']['successful_before_deadline'] / 2,
                           'record_path': str(root / 'calibration' / f'mix-{mix}-rep-{rep}' / 'record.json'),
                           'result': record['result']})
        median = statistics.median(t['completed_rate'] for t in trials)
        # 10% expected-count headroom is only an initial cap decision, never a
        # hidden truncation. Actual Poisson files must all pass the hard cap.
        baseline = min(median, CAP / (2 * max(FACTORS) * 1.10))
        attempts = []
        attempt = 0
        while True:
            candidates = []
            for factor, rep in itertools.product(FACTORS, range(REPETITIONS)):
                seed = args.ordering_seed + mix * 10000 + int(factor * 10) * 100 + rep
                path = root / 'schedules' / f'mix-{mix}-factor-{factor}-rep-{rep}-attempt-{attempt}.bin'
                spec = schedule_file(path, seed, mix, baseline * factor)
                spec.update({'factor': factor, 'rep': rep, 'path': str(path), 'sha256': digest(path)})
                candidates.append(spec)
            accepted = all(s['accepted'] for s in candidates)
            attempts.append({'baseline_rate': baseline, 'accepted': accepted, 'candidates': candidates})
            if accepted:
                schedules.extend(candidates)
                break
            baseline *= 0.8
            attempt += 1
            require(attempt < 20 and baseline > 0, 'unable to fit bounded schedules; no comparisons allowed')
        calibrations[str(mix)] = {'kind': 'native_only_closed_loop_continuous_two_seconds', 'trials': trials,
                                  'median_completed_rate': median, 'common_baseline_rate': baseline,
                                  'baseline_reduced_for_cap': baseline < median, 'cap': CAP,
                                  'initial_expected_count_headroom': 1.10, 'cap_attempts': attempts,
                                  'offered_rates': {str(f): baseline * f for f in FACTORS}}
    # Freeze before any comparative process. No backend-specific retuning.
    save(root / 'calibration.json', calibrations)
    freeze(files, root / 'calibration.json')
    for path in (root / 'schedules').iterdir():
        freeze(files, path)
    rng = random.Random(args.ordering_seed)
    trial_plan = []
    for rep in range(REPETITIONS):
        cells = [s for s in schedules if s['rep'] == rep]
        rng.shuffle(cells)
        for spec in cells:
            backends = list(BACKENDS)
            rng.shuffle(backends)
            for backend in backends:
                trial_plan.append({'id': f'trial-{len(trial_plan):03d}', 'backend': backend,
                                   'mix': spec['find_percent'], 'factor': spec['factor'], 'rep': rep,
                                   'seed': spec['seed'], 'schedule': spec,
                                   'command': command(builds[backend]['binary'], spec['find_percent'], spec['seed'], spec['path'])})
    require(len(trial_plan) == 90, 'incomplete predefined comparison matrix')
    manifest = {'schema': SCHEMA, 'experiment': EXPERIMENT, 'scope': SCOPE, 'created_at': utc(),
                'machine': receipt, 'expected_trials': 90, 'calibration_trials': 6, 'smoke_trials': 15,
                'seconds': 2, 'drain_seconds': 5, 'hard_grace_seconds': 1, 'process_timeout_seconds': 120,
                'memory_limit_gib': 32, 'preload': 100000, 'arrival_cap': CAP,
                'max_arrival_storage_bytes': CAP * 8, 'repetitions': REPETITIONS, 'ordering_seed': args.ordering_seed,
                'calibration': calibrations, 'builds': builds, 'trial_plan': trial_plan,
                'sources_archive': {'path': str(root / 'sources.tar.gz'), 'sha256': digest(root / 'sources.tar.gz')},
                'measurement': 'per-operation response/scheduling histograms and coarse watchdog, no fairness instrumentation; '
                               'same wrapper calibration and comparisons; no warmup; only immutable preload reads; '
                               'deadline counts use wrapper return, not frozen operation internal timestamp'}
    save(root / 'manifest.json', manifest)
    freeze(files, root / 'manifest.json')
    freeze(files, root / 'intent.json')
    assert_frozen(files)
    save(root / 'prepared.json', {'schema': SCHEMA, 'status': 'prepared', 'completed_at': utc(),
                                 'manifest_sha256': digest(root / 'manifest.json'), 'files': files})
    print('Prepared calibration and immutable 90-trial plan:', root)


def run(root):
    prepared = load(root / 'prepared.json')
    require(prepared['status'] == 'prepared', 'successful preparation required')
    assert_frozen(prepared['files'])
    manifest = load(root / 'manifest.json')
    machine_receipt = machine()
    (root / 'trials').mkdir()  # no overwrite/resume/retry or outcome-based selection
    save(root / 'run-start.json', {'started_at': utc(), 'machine': machine_receipt,
                                  'prepared_sha256': digest(root / 'prepared.json')})
    records = {}
    for item in manifest['trial_plan']:
        spec = item['schedule']
        directory = root / 'trials' / item['id']
        record = invoke(directory, item['command'], manifest['process_timeout_seconds'],
                        lambda result, x=item, s=spec: validate(result, x['mix'], x['seed'], s['generated']))
        records[item['id']] = {'success': record['success'], 'record_sha256': digest(directory / 'record.json')}
        print(item['id'], item['mix'], item['factor'], item['backend'],
              record['result'].get('status') if record['result'] else 'no-result', flush=True)
    # Avoid hashing large inputs between timed trials; boundary checks detect any
    # drift and invalidate the cohort rather than silently accepting changed code.
    integrity_error = None
    try:
        assert_frozen(prepared['files'])
    except (OSError, ValueError) as exc:
        integrity_error = str(exc)
    save(root / 'run-complete.json', {'completed_at': utc(), 'records': records,
                                     'input_integrity': integrity_error is None, 'integrity_error': integrity_error,
                                     'prepared_sha256': digest(root / 'prepared.json'),
                                     'successful': sum(r['success'] for r in records.values()), 'expected': 90})
    require(integrity_error is None, 'cohort invalidated by input drift: ' + str(integrity_error))


def summed_hist(result, metric, phases=('before_deadline', 'drain')):
    bins = [0] * 64
    for worker in result['workers']:
        hists = ([worker[metric]] if metric == 'unreturned_censor_age_ns' else
                 itertools.chain.from_iterable(worker[metric][phase] for phase in phases))
        for hist in hists:
            bins = [a + b for a, b in zip(bins, hist['bins'])]
    return bins


def quantile_bound(bins, fraction=0.99, lower=False):
    total = sum(bins)
    if total == 0:
        return None
    rank = math.ceil(total * fraction)
    seen = 0
    for index, count in enumerate(bins):
        seen += count
        if seen >= rank:
            if lower:
                return 0 if index == 0 else 2 ** (index - 1)
            return None if index == 63 else 0 if index == 0 else 2 ** index


def trial_row(item, record):
    spec = item['schedule']
    row = {'id': item['id'], 'mix': item['mix'], 'factor': item['factor'], 'rep': item['rep'],
           'backend': item['backend'], 'schedule_sha256': spec['sha256'], 'seed': item['seed'],
           'offered_rate': spec['generated'] / 2, 'target_rate': spec['rate_per_second'],
           'generated': spec['generated'], 'valid': record is not None and record['success'],
           'status': 'missing' if record is None else (record.get('result') or {}).get('status', 'process_failed'),
           'error': None if record is None else record.get('error', record.get('validation_error'))}
    result = None if record is None else record.get('result')
    row['raw_result'] = result  # retain every worker, histogram, oracle, partial failure and metric
    if not row['valid']:
        return row
    for key in ('submitted', 'returned', 'successful', 'submitted_before_deadline', 'returned_before_deadline',
                'successful_before_deadline', 'successful_during_drain', 'unsubmitted_at_deadline',
                'inflight_at_deadline', 'backlog_at_deadline', 'unsatisfied_at_deadline', 'unsubmitted_final',
                'unsatisfied_final', 'inflight_final', 'failed_returns', 'window_cpu_observation_ns', 'peak_rss_kib'):
        row[key] = result[key]
    row.update({'completed_rate': result['successful_before_deadline'] / 2,
                'completion_fraction_window': result['successful_before_deadline'] / spec['generated'] if spec['generated'] else 1,
                'completion_fraction_final': result['successful'] / spec['generated'] if spec['generated'] else 1,
                'window_cpu_seconds': result['window_process_cpu_ns'] / 1e9,
                'total_cpu_seconds': result['process_cpu_ns'] / 1e9,
                'drain_seconds': result['worker_drain_ns'] / 1e9,
                'join_drain_seconds': result['join_drain_ns'] / 1e9})
    returned = summed_hist(result, 'scheduled_to_return_ns')
    censor = summed_hist(result, 'unreturned_censor_age_ns')
    require(sum(returned) + sum(censor) == spec['generated'], 'saved record loses scheduled outcomes')
    row['scheduled_p99_returned_upper_ms'] = value_ms(quantile_bound(returned))
    row['scheduled_p99_all_outcomes_lower_ms'] = value_ms(quantile_bound([a + b for a, b in zip(returned, censor)], lower=True))
    row['scheduled_p99_is_censored'] = sum(censor) != 0
    row['response_p99_returned_upper_ms'] = value_ms(quantile_bound(summed_hist(result, 'submit_to_return_ns')))
    row['submit_lag_p99_returned_upper_ms'] = value_ms(quantile_bound(summed_hist(result, 'submit_lag_ns')))
    for phase in ('before_deadline', 'drain'):
        row[f'scheduled_p99_{phase}_upper_ms'] = value_ms(quantile_bound(summed_hist(result, 'scheduled_to_return_ns', (phase,))))
    return row


def value_ms(value):
    return None if value is None else value / 1e6


def compact_groups(rows):
    metrics = ('offered_rate', 'completed_rate', 'completion_fraction_window', 'completion_fraction_final',
               'scheduled_p99_returned_upper_ms', 'scheduled_p99_all_outcomes_lower_ms',
               'response_p99_returned_upper_ms', 'submit_lag_p99_returned_upper_ms', 'window_cpu_seconds',
               'total_cpu_seconds', 'backlog_at_deadline', 'unsubmitted_at_deadline', 'inflight_at_deadline',
               'unsatisfied_final', 'drain_seconds')
    groups = []
    for mix, factor, backend in itertools.product(MIXES, FACTORS, BACKENDS):
        subset = [r for r in rows if (r['mix'], r['factor'], r['backend']) == (mix, factor, backend)]
        valid = [r for r in subset if r['valid']]
        group = {'mix': mix, 'factor': factor, 'backend': backend, 'expected_trials': 3,
                 'successful_trials': len(valid), 'failed_trials': 3 - len(valid),
                 'incomplete_drains': sum(r['status'] == 'incomplete' for r in valid),
                 'backlogged_windows': sum(r['backlog_at_deadline'] > 0 for r in valid), 'metrics': {}}
        for metric in metrics:
            values = [r[metric] for r in valid if r.get(metric) is not None]
            group['metrics'][metric] = {'values': values, 'median': statistics.median(values) if values else None,
                                        'min': min(values) if values else None, 'max': max(values) if values else None}
        groups.append(group)
    return groups


def comparisons(rows):
    contrasts = []
    metrics = {'completed_rate': 'higher', 'window_cpu_seconds': 'lower',
               'total_cpu_seconds': 'lower', 'backlog_at_deadline': 'lower',
               'drain_seconds': 'lower', 'scheduled_p99_returned_upper_ms': 'lower',
               'response_p99_returned_upper_ms': 'lower'}
    lookup = {(r['mix'], r['factor'], r['backend'], r['rep']): r for r in rows}
    for mix, factor, baseline in itertools.product(MIXES, FACTORS, ('native', 'bridge_mutex', 'fc', 'uscl')):
        pairs = []
        for rep in range(REPETITIONS):
            pq, other = lookup[mix, factor, 'fc_pq', rep], lookup[mix, factor, baseline, rep]
            if pq['valid'] and other['valid']:
                require(pq['schedule_sha256'] == other['schedule_sha256'], 'unpaired schedule identity')
                full = pq['unsatisfied_final'] == other['unsatisfied_final'] == 0
                ratios, deltas = {}, {}
                for metric in metrics:
                    a, b = pq.get(metric), other.get(metric)
                    deltas[metric] = None if a is None or b is None else a - b
                    if a is None or b is None or ('p99' in metric and not full):
                        ratios[metric] = None
                    else:
                        ratios[metric] = a / b if b else 1.0 if a == 0 else None
                pairs.append({'rep': rep, 'fc_pq_minus_baseline': deltas,
                              'fc_pq_over_baseline': ratios, 'both_full_drain': full})
        classifications = {}
        for metric, preferred in metrics.items():
            values = [p['fc_pq_over_baseline'][metric] for p in pairs]
            if len(values) != 3 or any(v is None for v in values):
                label = 'inconclusive_missing_failed_censored_or_zero_denominator'
            elif all(v > 1.05 for v in values):
                label = 'consistent_increase'
            elif all(v < 0.95 for v in values):
                label = 'consistent_decrease'
            else:
                label = 'mixed_or_small_not_equivalence'
            favorable = ((label == 'consistent_increase' and preferred == 'higher') or
                         (label == 'consistent_decrease' and preferred == 'lower'))
            unfavorable = ((label == 'consistent_decrease' and preferred == 'higher') or
                           (label == 'consistent_increase' and preferred == 'lower'))
            classifications[metric] = {'classification': label, 'preferred_direction': preferred,
                                       'observed_direction': 'favorable' if favorable else 'unfavorable' if unfavorable else 'undetermined',
                                       'ratios': values}
        contrasts.append({'mix': mix, 'factor': factor, 'baseline': baseline,
                          'classification': classifications['completed_rate']['classification'],
                          'metrics': classifications, 'pairs': pairs,
                          'interpretation': 'Predeclared all-three >1.05/<0.95 rule per metric, not significance or equivalence. '
                          'Offered-demand-constrained completion ties do not remove CPU/tail tradeoffs. '
                          'Total CPU includes unequal drain horizons. No pure-overhead versus scheduling causal attribution.'})
    return contrasts


def plots(directory, rows, groups):
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    colors = dict(zip(BACKENDS, ('#444444', '#9467bd', '#1f77b4', '#d62728', '#2ca02c')))
    for mix in MIXES:
        fig, axes = plt.subplots(2, 3, figsize=(16, 9), constrained_layout=True)
        metrics = [('completed_rate', 'Completed by 2s / second'),
                   ('scheduled_p99_returned_upper_ms', 'Scheduled p99 upper bound, returned calls (ms)'),
                   ('scheduled_p99_all_outcomes_lower_ms', 'Scheduled p99 lower bound, ALL arrivals (ms)'),
                   ('window_cpu_seconds', 'CPU seconds through ~2s deadline (sampled)'),
                   ('backlog_at_deadline', 'Unreturned at 2s (unsubmitted + inflight)'),
                   ('drain_seconds', 'Worker drain after 2s (seconds)')]
        for axis, (metric, label) in zip(axes.flat, metrics):
            for backend in BACKENDS:
                points = sorted((g for g in groups if g['mix'] == mix and g['backend'] == backend), key=lambda g: g['factor'])
                x = [g['metrics']['offered_rate']['median'] for g in points]
                y = [g['metrics'][metric]['median'] for g in points]
                usable = [(a, b) for a, b in zip(x, y) if a is not None and b is not None]
                axis.plot([p[0] for p in usable], [p[1] for p in usable], 'o-', color=colors[backend], label=backend)
                points_raw = [r for r in rows if r['mix'] == mix and r['backend'] == backend and r['valid'] and r.get(metric) is not None]
                axis.scatter([r['offered_rate'] for r in points_raw], [r[metric] for r in points_raw], color=colors[backend], alpha=.4, s=18)
            if metric == 'completed_rate':
                upper = max((r['offered_rate'] for r in rows if r['mix'] == mix), default=1)
                axis.plot([0, upper], [0, upper], '--', color='black', alpha=.5, label='offered=completed')
            if 'p99' in metric:
                axis.set_yscale('symlog', linthresh=.001)
            axis.set_xlabel('Realized common offered arrivals / second')
            axis.set_ylabel(label)
            axis.grid(alpha=.2)
        axes[0, 0].legend(fontsize=8)
        failures = sum(not r['valid'] for r in rows if r['mix'] == mix)
        incomplete = sum(r['status'] == 'incomplete' for r in rows if r['mix'] == mix)
        fig.suptitle(f'SYNTHETIC Poisson, find/insert {mix}/{100-mix}; n=3, points + medians, no CIs\n'
                     f'0.3/0.7/1.1 x frozen native baseline; failures={failures}, incomplete drains={incomplete}; returned-only tail is conditional')
        fig.savefig(directory / f'mix-{mix}-rates.png', dpi=160)
        plt.close(fig)
        fig, axes = plt.subplots(1, 3, figsize=(15, 4.5), constrained_layout=True)
        for axis, (metric, label) in zip(axes, (('total_cpu_seconds', 'CPU seconds through joins (INCLUDING drain)'),
                                              ('completion_fraction_window', 'Successful / ALL arrivals by 2s'),
                                              ('completion_fraction_final', 'Successful / ALL arrivals after bounded drain'))):
            for backend in BACKENDS:
                subset = sorted((g for g in groups if g['mix'] == mix and g['backend'] == backend), key=lambda g: g['factor'])
                points = [(g['factor'], g['metrics'][metric]['median']) for g in subset if g['metrics'][metric]['median'] is not None]
                axis.plot([p[0] for p in points], [p[1] for p in points], 'o-', color=colors[backend], label=backend)
                raw = [r for r in rows if r['mix'] == mix and r['backend'] == backend and r['valid']]
                axis.scatter([r['factor'] for r in raw], [r[metric] for r in raw], color=colors[backend], alpha=.4, s=18)
            axis.set_xlabel('Factor of frozen native baseline (NOT backend utilization)')
            axis.set_ylabel(label)
            axis.set_xticks(FACTORS)
            axis.grid(alpha=.2)
        axes[0].legend(fontsize=8)
        fig.suptitle(f'Synthetic {mix}/{100-mix}: bounded horizons; total CPU is not a fixed-time efficiency comparison')
        fig.savefig(directory / f'mix-{mix}-outcomes.png', dpi=160)
        plt.close(fig)
    return matplotlib.__version__


def analyze(root):
    manifest = load(root / 'manifest.json')
    prepared = load(root / 'prepared.json')
    require(digest(root / 'manifest.json') == prepared['manifest_sha256'], 'saved manifest changed')
    directory = root / 'analysis'
    directory.mkdir()  # saved-data-only and refuse existing analysis
    complete = load(root / 'run-complete.json') if (root / 'run-complete.json').exists() else None
    cohort_integrity = complete is not None and complete.get('input_integrity', False)
    if complete is not None:
        require(digest(root / 'prepared.json') == complete['prepared_sha256'], 'saved preparation identity changed')
    rows = []
    raw_hashes = {}
    for item in manifest['trial_plan']:
        path = root / 'trials' / item['id'] / 'record.json'
        record = load(path) if path.exists() else None
        if record is not None:
            raw_hashes[item['id']] = digest(path)
            if complete is not None:
                require(raw_hashes[item['id']] == complete['records'][item['id']]['record_sha256'], 'raw record changed')
            for name in ('stdout', 'stderr'):
                require(digest(path.parent / name) == record[name + '_sha256'], 'raw output changed')
            if record['success']:
                validate(record['result'], item['mix'], item['seed'], item['schedule']['generated'])
            if not cohort_integrity:
                record = {**record, 'success': False, 'validation_error': 'cohort incomplete or final input integrity unverified'}
        rows.append(trial_row(item, record))
    groups = compact_groups(rows)
    contrasts = comparisons(rows)
    summary = {'schema': SCHEMA, 'experiment': EXPERIMENT, 'scope': SCOPE, 'created_at': utc(),
               'expected_trials': 90, 'successful_trials': sum(r['valid'] for r in rows),
               'failed_trials': sum(not r['valid'] for r in rows), 'missing_trials': sum(r['status'] == 'missing' for r in rows),
               'incomplete_drain_trials': sum(r['status'] == 'incomplete' for r in rows),
               'manifest_sha256': digest(root / 'manifest.json'), 'raw_record_hashes': raw_hashes,
               'calibration': manifest['calibration'], 'groups': groups, 'trials': rows, 'comparisons': contrasts,
               'latency_notes': 'Power-of-two histogram bounds, no interpolation. Returned-call p99 is conditional when incomplete. '
                                'All-arrival p99 lower bound uses returned latency plus unreturned censor ages at joins; '
                                'it is not an exact p99 or a completion-only success denominator. Failed records retain raw metrics '
                                'but are not silently used as valid comparisons. Hard timeout has no race-free histograms.',
               'cpu_notes': 'Window CPU is a process sample taken at or just after 2s; exact offset retained. '
                            'Total CPU covers release through joins, excludes preload/oracle, and includes unequal drain horizons. '
                            'No CPU-per-op causal pure-overhead claim.',
               'mechanism_notes': 'Backlog plus zero completions does not establish idle. Scheduling redistribution versus overhead '
                                  'cannot be isolated without separate callback occupancy/service evidence. No fairness claim.'}
    summary['matplotlib_version'] = plots(directory, rows, groups)
    save(directory / 'summary.json', summary)
    columns = sorted(set().union(*(set(r) for r in rows)) - {'raw_result'})
    with (directory / 'trials.csv').open('x', newline='') as target:
        writer = csv.DictWriter(target, fieldnames=columns)
        writer.writeheader()
        writer.writerows({k: v for k, v in r.items() if k != 'raw_result'} for r in rows)
    lines = [EXPERIMENT, SCOPE, '', f"Expected90; successful {summary['successful_trials']}; failed/missing {summary['failed_trials']}; "
             f"incomplete bounded drains {summary['incomplete_drain_trials']}.",
             'All predefined cases retained. Failure is not a slow completed trial; absence of evidence is not equivalence.',
             'No warmup. Each independent process starts with100k immutable read keys; unique inserts grow the DB.',
             'No exact saturation assumption: all rate factors refer to frozen native closed-loop calibration, capped if needed.',
             'Overload indicators are incomplete all-arrival service by the deadline and bounded-drain exhaustion, not proof of lock idle.',
             'A few natural deadline-straddling calls alone do not establish sustained overload. See every backlog and success fraction.',
             'Cohort final input integrity verified: ' + str(cohort_integrity), '']
    for mix in MIXES:
        c = manifest['calibration'][str(mix)]
        lines.append(f"Mix{mix}/{100-mix}: native median={c['median_completed_rate']:.3f}/s; common baseline={c['common_baseline_rate']:.3f}/s; "
                     f"cap reduction={c['baseline_reduced_for_cap']}; offered={c['offered_rates']}")
    lines += ['', 'Observed FC-PQ paired metrics (all3 ratios >1.05 increase; all3 <0.95 decrease; otherwise mixed/small, not equivalence):']
    for contrast in contrasts:
        lines.append(f"mix{contrast['mix']} factor{contrast['factor']} vs{contrast['baseline']}: {contrast['metrics']}; paired metrics={contrast['pairs']}")
    lines += ['', 'All cells: deadline backlog and bounded drain outcomes (backlog alone is not a lock-idle proof):']
    for group in groups:
        lines.append(f"mix{group['mix']} factor{group['factor']} {group['backend']}: valid{group['successful_trials']}/3; "
                     f"backlogged windows{group['backlogged_windows']}; incomplete drains{group['incomplete_drains']}; "
                     f"completed/s={group['metrics']['completed_rate']['values']}; "
                     f"backlog={group['metrics']['backlog_at_deadline']['values']}; drain(s)={group['metrics']['drain_seconds']['values']}")
    lines += ['', summary['latency_notes'], summary['cpu_notes'], summary['mechanism_notes'],
              'Plots expose offered/completed rate, both tail bounds, CPU, backlog, drain and all-arrival success fractions.',
              'Scheduling effects and instrumentation/pacing costs remain part of this synthetic workload; no production-realism claim.']
    (directory / 'report.txt').write_text('\n'.join(lines) + '\n')
    print('Saved analysis:', directory)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group(required=True)
    modes.add_argument('--prepare-only', action='store_true')
    modes.add_argument('--run', action='store_true')
    modes.add_argument('--analyze-only', action='store_true')
    parser.add_argument('--output-root', required=True, type=Path)
    parser.add_argument('--binary-root', type=Path, default=FROZEN, help='frozen joined-build inputs, never rebuilt')
    parser.add_argument('--ordering-seed', type=int, default=2026092440)
    args = parser.parse_args()
    args.output_root = args.output_root.absolute()
    args.binary_root = args.binary_root.absolute()
    require(0 <= args.ordering_seed < 2**62, 'seed out of range')
    if args.prepare_only:
        prepare(args)
    elif args.run:
        run(args.output_root)
    else:
        analyze(args.output_root)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as exc:
        sys.exit(type(exc).__name__ + ': ' + str(exc))
