#!/usr/bin/env python3
"""Prepare, run or analyze the frozen actual-UpScaleDB table matrix (180 trials by default).

Caller must hold the cohort measurement lock (shared for prepare/analyze,
exclusive for run), tables.lock, and taskset CPU56-63. No implicit run mode.
"""
import argparse
import csv
from datetime import datetime, timezone
import hashlib
import itertools
import json
import math
import os
from pathlib import Path
import platform
import random
import re
import resource
import shlex
import shutil
import signal
import statistics
import subprocess
import sys
import time

import build as builder

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
FROZEN_ROOT = ROOT / '.worktree/upscaledb'
BUILD_ROOT = FROZEN_ROOT
CONTROL_BACKENDS = ('native', 'bridge_mutex', 'fc', 'fc_pq', 'uscl')
BACKENDS = (*CONTROL_BACKENDS, 'cfl_local', 'spinlock', 'mcs', 'ticket', 'clh')
BACKEND_IDS = {backend: index for index, backend in enumerate(BACKENDS[1:])}
SAFETY_EXCLUSIONS = {
    'shfl': 'Excluded before preparation: RawShflLock overlaps AtomicU32/AtomicU8 accesses '
            '(crates/libdlock/src/dlock2/shfl_lock/lock.rs:162-163,309-312,358-416) and writes '
            'nid through shared &QNode without UnsafeCell (lines302,323). No algorithm repair or substitution.',
}
BACKEND_LABELS = {'native': 'Native', 'bridge_mutex': 'Borrowed mutex', 'fc': 'FC', 'fc_pq': 'FC-PQ',
                  'uscl': 'USCL', 'cfl_local': 'CFL local proxy', 'spinlock': 'SpinLock',
                  'mcs': 'MCS', 'ticket': 'Ticket', 'clh': 'CLH'}
BACKEND_COLORS = ('#333333', '#777777', '#1f77b4', '#d62728', '#9467bd', '#8c564b',
                  '#2ca02c', '#ff7f0e', '#e377c2', '#17becf')
LAYOUTS = ('coarse', 'split')
TABLES = (1, 8, 32)
CPUS = list(range(56, 64))
SCHEMA = 'boundary-tables-v1'
UNSET_ENV = ('NIX_CFLAGS_COMPILE', 'NIX_LDFLAGS', 'LD_PRELOAD', 'LD_LIBRARY_PATH', 'LD_AUDIT')
CAP = 32 << 30
HYPOTHESES = [
    'Independent environments may reduce contention as K grows.',
    'USCL per-lock reservations may impede requesters moving between tables.',
    'FC and FC-PQ may lose combining benefit or incur per-lock metadata/coordination overhead.',
    'K=1 coarse and split are implementation controls with the same resource layout.',
]
CAVEATS = [
    'Three independent seeded repetitions: points and observed ranges only, no confidence intervals or equivalence claims.',
    'Initial total records is fixed at 32768; final records grow with successful unique inserts and differ with throughput.',
    'Split changes environment caches, allocator state and metadata organization as well as gate count; not pure lock-granularity causality.',
    'Closed-loop alternating find/unique-insert model, not a production trace. Identical seeded per-worker request streams; timed prefixes differ.',
    'Every worker performs 8 find-only sweeps of every table on the same live threads before the gate; no warmup inserts.',
    'No per-request shared atomic instrumentation, callback occupancy, service-time fairness or causal idle-with-pending-work claim.',
    'Requester latency surrounds guarded API calls and includes drain; throughput counts completions within the 2-second window.',
    'Process CPU includes gate release, worker loops, drain, joins and TLS destructors; excludes setup, warmup and final oracle.',
    'Log2-histogram quantiles are bin upper bounds, not exact quantiles; unavailable below conservative sample thresholds.',
    'At most 4000000 inserts per worker / 32000000 total; reaching cap fails rather than silently truncating a trial.',
    '32GiB is a virtual address-space cap, not a resident-memory cap. Process watchdog covers setup, warmup, measurement and oracle.',
    'CPU56-63/node1 bind; SMT siblings120-127 reserved but not OS-isolated. Unrelated host interference remains possible.',
    'USCL uses frozen equal weight1024 and 4800000 TSC-cycle nominal slice; no modern scheduler/CFS-era fidelity claim.',
    'cfl_local is the repository local proxy, not the published CFL implementation; no fidelity claim.',
    'K=1 coarse/split have the same environment and lock count; variation is a control, not a granularity benefit.',
    'All selected backends use contemporaneous harness builds; bridge variants share the newly rebuilt Rust archive.',
    'Do not compare absolute rates to other boundary experiments on different allocated CPUs.',
]


def utc():
    return datetime.now(timezone.utc).isoformat()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def load(path):
    def invalid(value):
        raise ValueError('nonfinite JSON value: ' + value)
    return json.loads(Path(path).read_text(), parse_constant=invalid)


def save(path, value):
    with Path(path).open('x') as target:
        json.dump(value, target, indent=2, sort_keys=True, allow_nan=False)
        target.write('\n')


def freeze(files, path, expected=None):
    path = str(Path(path).absolute())
    actual = digest(path)
    require(expected is None or actual == expected, 'frozen hash mismatch: ' + path)
    require(path not in files or files[path] == actual, 'input changed: ' + path)
    files[path] = actual


def assert_frozen(files):
    for path, expected in files.items():
        require(digest(path) == expected, 'prepared file changed: ' + path)


def cpu_ranges(text):
    result = set()
    for piece in text.strip().split(','):
        if piece:
            bounds = piece.split('-')
            result.update(range(int(bounds[0]), int(bounds[-1]) + 1))
    return result


def machine(cpus=CPUS):
    status = dict(line.split(':', 1) for line in Path('/proc/self/status').read_text().splitlines() if ':' in line)
    topology = []
    for cpu in cpus:
        base = Path('/sys/devices/system/cpu') / ('cpu' + str(cpu))
        topology.append({'cpu': cpu, 'core': int((base / 'topology/core_id').read_text()),
                     'socket': int((base / 'topology/physical_package_id').read_text()),
                     'siblings': sorted(cpu_ranges((base / 'topology/thread_siblings_list').read_text())),
                     'nodes': sorted(int(p.name[4:]) for p in base.glob('node[0-9]*'))})
    receipt = {'timestamp': utc(), 'hostname': platform.node(), 'uname': list(platform.uname()),
               'affinity': sorted(os.sched_getaffinity(0)), 'cpus': topology,
               'mems_allowed': status.get('Mems_allowed_list', '').strip(),
               'python': sys.version, 'python_executable': sys.executable,
               'numa_balancing': Path('/proc/sys/kernel/numa_balancing').read_text().strip()}
    require(receipt['affinity'] == cpus, 'controller must run under taskset -c' + str(cpus[0]) + '-' + str(cpus[-1]))
    require(1 in cpu_ranges(receipt['mems_allowed']), 'NUMA node1 unavailable')
    require(all(c['nodes'] == [1] for c in topology), 'CPUs must be on node1')
    require(len({(c['socket'], c['core']) for c in topology}) == len(cpus), 'CPUs must be distinct physical cores')
    require(all(c['siblings'] == [c['cpu'], c['cpu'] + 64] for c in topology), 'SMT reservation differs from frozen allocation')
    return receipt


def child_limit():
    _, hard = resource.getrlimit(resource.RLIMIT_AS)
    if hard != resource.RLIM_INFINITY and hard < CAP:
        raise RuntimeError('inherited hard address-space cap below 32GiB')
    resource.setrlimit(resource.RLIMIT_AS, (CAP, hard))


def interrupt(signum, frame):
    raise InterruptedError('received signal ' + str(signum))


def invoke(directory, command, timeout, validator=None):
    """Retain raw bytes, partial JSON, exit, timeout and hashes, even on failure."""
    directory.mkdir()
    record = {'schema': SCHEMA, 'command': command, 'cwd': str(ROOT), 'started_at': utc(),
              'timeout_seconds': timeout, 'address_space_cap_bytes': CAP,
              'environment_unset': list(UNSET_ENV), 'success': False, 'returncode': None,
              'timed_out': False, 'result': None}
    save(directory / 'invocation.json', dict(record))
    process = None
    interrupted = False
    started = time.monotonic_ns()
    with (directory / 'stdout').open('xb') as stdout, (directory / 'stderr').open('xb') as stderr:
        try:
            env = {key: value for key, value in os.environ.items() if key not in UNSET_ENV}
            process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=stdout, stderr=stderr,
                                       start_new_session=True, preexec_fn=child_limit)
            record['pid'] = process.pid
            record['returncode'] = process.wait(timeout=timeout)
            stdout.flush()
            if validator is not None:
                record['result'] = load(directory / 'stdout')
            require(record['returncode'] == 0, 'nonzero process exit: ' + str(record['returncode']))
            if validator is not None:
                validator(record['result'])
            record['success'] = True
        except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError, KeyboardInterrupt) as exc:
            record['error'] = type(exc).__name__ + ': ' + str(exc)
            record['timed_out'] = isinstance(exc, subprocess.TimeoutExpired)
            interrupted = isinstance(exc, (KeyboardInterrupt, InterruptedError))
        finally:
            if process is not None:
                if process.poll() is None or record['timed_out']:
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                record['returncode'] = process.wait()
            record['finished_at'] = utc()
            record['wall_elapsed_ns'] = time.monotonic_ns() - started
    for name in ('stdout', 'stderr', 'invocation.json'):
        record[name + '_sha256'] = digest(directory / name)
    if record['result'] is None and validator is not None:
        try:
            record['result'] = load(directory / 'stdout')
        except (OSError, ValueError):
            pass
    save(directory / 'record.json', record)
    if interrupted:
        raise InterruptedError('interrupted invocation retained at ' + str(directory))
    return record


def command(binary, layout, tables, seed, seconds=2, errors=False, workers=None, routing=None):
    result = [str(binary), '--layout', layout, '--tables', str(tables), '--seed', str(seed), '--seconds', str(seconds)]
    if workers is not None:
        result += ['--workers', str(workers)]
    if routing is not None:
        result += ['--routing', routing]
    return result + ['--self-test', 'errors'] if errors else result


def validate(result, item, seconds=2, errors=False):
    require(result['schema'] == SCHEMA and not result['failed'], 'harness reports failure')
    for field in ('backend', 'layout', 'tables', 'seed'):
        require(result[field] == item[field], 'result ' + field + ' mismatch')
    workers = item.get('workers', 8)
    cpus = list(range(64 - workers, 64))
    require(result.get('workers_count', workers) == workers, 'worker count mismatch')
    require(result.get('routing', 'uniform') == item.get('routing', 'uniform'), 'routing mismatch')
    require(result.get('api_overlap_diagnostic', False) == item.get('diagnostic', False),
            'diagnostic build mismatch')
    require(all(value == 0 for value in result['shutdown'].values()), 'shutdown failure')
    require(len(result['verification']) == item['tables'], 'oracle table count mismatch')
    for table, v in enumerate(result['verification']):
        require(v['table'] == table, 'oracle table id mismatch')
        require(v['seen'] == v['expected'] == v['db_count'], 'oracle exact count mismatch')
        require(not any(v[field] for field in ('bad', 'integrity_status', 'count_status', 'cursor_status')), 'oracle failure')
    if errors:
        require(result['self_test'] == 'errors' and result['checks'] == 4 * item['tables'], 'error-status smoke incomplete')
        require(result['probe_inserts'] == item['tables'], 'probe insert count mismatch')
        require(all(v['expected'] == 32768 // item['tables'] + 1 for v in result['verification']), 'probe oracle mismatch')
        return
    require(result['seconds'] == seconds and result['workers_count'] == workers, 'measurement shape mismatch')
    require(result['initial_records'] == 32768 and result['max_inserts_total'] == workers * 4000000,
            'dataset cap mismatch')
    require(result['environments'] == (1 if item['layout'] == 'coarse' else item['tables']), 'environment mapping mismatch')
    require(result['warmup']['inserts'] == 0, 'untracked warmup inserts')
    require(result['memory_policy'] == {'requested': 'bind', 'effective': 'bind', 'nodes': [1],
                                        'init_cpu': cpus[0]}, 'NUMA binding mismatch')
    require(result['resources']['address_space_cap_bytes'] == CAP, 'address-space cap mismatch')
    require(result['deadline_ns'] - result['start_ns'] == int(seconds * 1e9), 'window mismatch')
    require(result['join_end_ns'] >= result['deadline_ns'], 'short successful window')
    require(result['drain_ns'] == result['join_end_ns'] - result['deadline_ns'], 'drain accounting mismatch')
    require(result['numa_maps_after_join_before_oracle'] and not result['numa_snapshot_error'], 'NUMA snapshot absent')
    require(len(result['workers']) == workers, 'worker count mismatch')
    inserted = [0] * item['tables']
    for worker_id, worker in enumerate(result['workers']):
        require(worker['id'] == worker_id and worker['requested_cpu'] == cpus[worker_id], 'worker identity mismatch')
        require(worker['cpu_start'] == worker['cpu_end'] == cpus[worker_id] and
                worker['singleton_start'] and worker['singleton_end'], 'worker singleton affinity failure')
        require(not worker['error'] and not any(worker[field] for field in ('db_error', 'bridge_error', 'bad_buffers')), 'worker error')
        require(worker['warmup_reads'] == 8 * item['tables'], 'live-worker warmup incomplete')
        require(len(worker['tables']) == item['tables'], 'progress table count mismatch')
        completed = [0, 0]
        late = 0
        for table_id, table in enumerate(worker['tables']):
            require(table['table'] == table_id, 'progress table identity mismatch')
            for role_id, role in enumerate(('finds', 'inserts')):
                require(0 <= table[role + '_on_time'] <= table[role], 'completion accounting mismatch')
                completed[role_id] += table[role]
                late += table[role] - table[role + '_on_time']
            inserted[table_id] += table['inserts']
        require(completed[0] in (completed[1], completed[1] + 1), 'alternating 50/50 stream mismatch')
        require(completed[1] <= result['max_inserts_total'] // result['workers_count'] and late <= 1,
                'per-worker insert/drain cap exceeded')
        require(worker['end_ns'] >= result['deadline_ns'], 'worker ended before deadline')
        for role_id, hist in enumerate(worker['latency']):
            require(len(hist['bins']) == 64 and all(isinstance(v, int) and v >= 0 for v in hist['bins']), 'invalid histogram')
            require(hist['samples'] == sum(hist['bins']) == completed[role_id], 'requester histogram count mismatch')
            require(hist['sum_ns'] >= hist['max_ns'] >= 0, 'latency summary mismatch')
    for table_id, count in enumerate(inserted):
        require(result['verification'][table_id]['expected'] == 32768 // item['tables'] + count, 'final expected set mismatch')


def workload_seeds(shuffle_seed):
    # Preserve the old five-backend workload seeds, independent of the new
    # cohort size/order. The original RNG also consumed one shuffle per rep.
    rng = random.Random(shuffle_seed)
    seeds = []
    for _ in range(3):
        seeds.append(rng.randrange(1, 2**63))
        cells = list(itertools.product(LAYOUTS, TABLES, CONTROL_BACKENDS))
        rng.shuffle(cells)
    return seeds


def matrix_backends(manifest):
    require(manifest['schema'] == SCHEMA, 'unexpected manifest schema')
    backends = tuple(manifest['backends'])
    require(backends and len(set(backends)) == len(backends) and set(backends) <= set(BACKENDS),
            'invalid predeclared backend set')
    excluded = manifest['excluded_backends']
    require(set(excluded) == (set(BACKENDS) | set(SAFETY_EXCLUSIONS)) - set(backends) and
            all(isinstance(reason, str) and reason.strip() for reason in excluded.values()),
            'every excluded backend requires a predeclared reason')
    require(manifest['repetitions'] == 3 and manifest['seconds'] == 2, 'unexpected workload dimensions')
    cells = set(itertools.product(range(1, 4), LAYOUTS, TABLES, backends))
    schedule = manifest['schedule']
    require(manifest['expected_trials'] == len(schedule) == len(cells) and
            {(s['rep'], s['layout'], s['tables'], s['backend']) for s in schedule} == cells,
            'unexpected manifest schedule')
    require(manifest['smoke_invocations'] == 2 * len(LAYOUTS) * len(TABLES) * len(backends),
            'unexpected gate count')
    require(set(manifest['builds']) == set(backends), 'build/backend set mismatch')
    seeds = workload_seeds(manifest['shuffle_seed'])
    require(manifest['workload_seeds'] == seeds, 'workload seeds changed')
    for index, item in enumerate(schedule):
        require(item['id'] == 'trial-%03d' % (index + 1) and item['seed'] == seeds[item['rep'] - 1],
                'trial identity or paired workload seed mismatch')
        require(item['command'] == command(manifest['builds'][item['backend']]['binary'],
                                           item['layout'], item['tables'], item['seed']),
                'scheduled workload command differs')
    return backends


def prepare(args, experiment=None):
    root = args.output_root
    backends = tuple(args.backends)
    require(backends and len(set(backends)) == len(backends), 'backend selection must be nonempty and unique')
    omitted = set(BACKENDS) - set(backends)
    require(not omitted or (args.exclusion_reason and args.exclusion_reason.strip()),
            '--exclusion-reason is required for a reduced backend set; no implicit failure exclusions')
    excluded = {**SAFETY_EXCLUSIONS, **{backend: args.exclusion_reason for backend in sorted(omitted)}}
    smoke_invocations = (2 * len(LAYOUTS) * len(TABLES) * len(backends) if experiment is None else
                         len(backends) * len(experiment['workers']) * (len(experiment['routing']) + 1) +
                         len(experiment['diagnostic_backends']) * (len(experiment['routing']) + 1))
    root.mkdir(parents=True, exist_ok=False)
    for directory in ('bin', 'prepare', 'sources'):
        (root / directory).mkdir()
    save(root / 'request.json', {'schema': SCHEMA if experiment is None else experiment['schema'],
                                'created_at': utc(), 'binary_root': str(args.binary_root),
                               'backends': backends, 'excluded_backends': excluded,
                               'shuffle_seed': args.shuffle_seed, 'timeout_seconds': args.timeout_seconds,
                               'experiment': experiment['name'] if experiment else 'other-locks'})
    receipt = machine(experiment['cpus'] if experiment else CPUS)
    files = {}
    sources = ['boundary_tables.py', 'boundary_tables.cc', 'build.py', 'native_harness.cc',
               'bridge.h', 'private_ops.h', 'bridge-ops.patch']
    if experiment is not None:
        sources.append('high_contention.py')
    for name in sources:
        source = HERE / name
        freeze(files, source)
        shutil.copyfile(source, root / 'sources' / name)
        freeze(files, root / 'sources' / name, files[str(source)])
    rust_sources_sha256 = builder.rust_sources_digest()
    for source in builder.rust_sources_files():
        freeze(files, source)
        copy = root / 'sources/rust' / source.relative_to(ROOT)
        copy.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, copy)
        freeze(files, copy, files[str(source)])
    frozen_builds = {}
    for backend in ('native', 'fc'):
        path = args.frozen_root / ('build-' + backend + '.json')
        freeze(files, path)
        frozen_builds[backend] = load(path)
        require(frozen_builds[backend]['schema'] == 2 and frozen_builds[backend]['variant'] == backend and
                frozen_builds[backend]['pinned_sha'] == builder.SHA, 'unexpected frozen upstream baseline')
    freeze(files, sys.executable)
    ldd = shutil.which('ldd')
    require(ldd is not None, 'ldd required for runtime dependency provenance')
    freeze(files, ldd)
    builds = {}
    shared_rust = None
    for backend in backends:
        archive_path = args.binary_root / ('build-' + backend + '.json')
        freeze(files, archive_path)
        archive = load(archive_path)
        require(archive['schema'] == 2 and archive['variant'] == backend and archive['pinned_sha'] == builder.SHA,
                'unexpected current build manifest')
        reference = frozen_builds['native' if backend == 'native' else 'fc']
        for field in ('source', 'source_kind', 'source_verification', 'library_verification', 'upstream', 'nixpkgs'):
            require(archive[field] == reference[field], 'upstream DB provenance changed: ' + field)
        for field in ('cc', 'cxx', 'cc_sha256', 'cxx_sha256', 'gcc_store', 'boost_store', 'boost_dev_store',
                      'cflags', 'cxxflags', 'ldflags', 'nix_cflags_compile_unset', 'nix_ldflags_unset'):
            require(archive['toolchain'][field] == reference['toolchain'][field], 'frozen compiler/flags changed: ' + field)
        for name, field in (('bridge.h', 'bridge_h_sha256'), ('private_ops.h', 'private_ops_sha256'),
                            ('native_harness.cc', 'harness_sha256'), ('build.py', 'build_py_sha256')):
            freeze(files, HERE / name, archive['hashes'][field])
        freeze(files, HERE / 'private_ops.h', reference['hashes']['private_ops_sha256'])
        if backend != 'native':
            freeze(files, HERE / 'bridge-ops.patch', reference['source_verification']['bridge_patch_sha256'])
        library = archive['library_verification']
        freeze(files, library['shared_object'], library['shared_object_sha256'])
        freeze(files, archive['binary'], archive['hashes']['binary_sha256'])
        source = Path(archive['source'])
        freeze(files, source / 'src/5upscaledb/upscaledb.cc', archive['source_verification']['implementation_sha256'])
        for name, field in (('config.status', 'config_status_sha256'), ('config.h', 'config_h_sha256')):
            freeze(files, source / name, library[field])
        for name, field in (('configure', 'configure_sha256'), ('config.h.in', 'config_h_in_sha256')):
            freeze(files, source / name, reference['hashes'][field])
        rust = archive['rust_staticlib']
        if backend == 'native':
            require(rust is None, 'native must not link a Rust bridge')
        else:
            require(rust['source_root'] == str(ROOT), 'Rust bridge must be rebuilt from the current worktree')
            require(rust['rust_sources_sha256'] == rust_sources_sha256 and rust['features'] == [] and
                    rust['bridge_h_sha256'] == digest(HERE / 'bridge.h'), 'Rust source/features/ABI mismatch')
            require(rust['compiler'] == reference['rust_staticlib']['compiler'], 'frozen Rust compiler changed')
            require(Path(rust['archive']).is_relative_to(args.binary_root), 'Rust archive must come from the fresh build root')
            require(shared_rust is None or shared_rust == rust, 'bridge variants do not share the same new Rust build')
            shared_rust = rust
            freeze(files, rust['archive'], rust['archive_sha256'])
        compile_command = list(archive['build']['harness_command'])
        freeze(files, compile_command[0], archive['toolchain']['cxx_sha256'])
        original_sources = [arg for arg in compile_command if arg.endswith('/native_harness.cc')]
        require(original_sources == [str(HERE / 'native_harness.cc')] and compile_command.count('-o') == 1,
                'compile command must use the current harness exactly once')
        require(compile_command[compile_command.index('-o') + 1] == archive['binary'], 'unexpected archived output')
        expected_command = list(reference['build']['harness_command'])
        old_harness = next(arg for arg in expected_command if arg.endswith('/native_harness.cc'))
        substitutions = {old_harness: str(HERE / 'native_harness.cc'),
                         '-I' + str(Path(old_harness).parent): '-I' + str(HERE),
                         reference['binary']: archive['binary']}
        if backend != 'native':
            substitutions['-DUPS_BRIDGE_KIND=1'] = '-DUPS_BRIDGE_KIND=' + str(BACKEND_IDS[backend])
            substitutions[reference['rust_staticlib']['archive']] = rust['archive']
        require(compile_command == [substitutions.get(arg, arg) for arg in expected_command],
                'current harness command differs from frozen compiler/flags/libraries')
        binary = root / 'bin' / ('boundary-tables-' + backend)
        # Exactly source/output substitutions; compiler, flags, include dirs and libraries stay frozen.
        compile_command[compile_command.index(original_sources[0])] = str(HERE / 'boundary_tables.cc')
        compile_command[compile_command.index('-o') + 1] = str(binary)
        builds[backend] = {'archived_manifest_path': str(archive_path), 'archived_manifest': archive,
                           'command': compile_command, 'binary': str(binary)}
    if experiment is not None:
        for backend in experiment['diagnostic_backends']:
            original = builds[backend]
            diagnostic = root / 'bin' / ('boundary-tables-' + backend + '-diagnostic')
            compile_command = list(original['command'])
            compile_command[compile_command.index('-o') + 1] = str(diagnostic)
            compile_command.insert(1, '-DUPS_API_OVERLAP_DIAGNOSTIC=1')
            builds[backend + '-diagnostic'] = {'archived_manifest_path': original['archived_manifest_path'],
                'archived_manifest': original['archived_manifest'], 'command': compile_command,
                'binary': str(diagnostic), 'backend': backend, 'diagnostic': True}
    rng = random.Random(args.shuffle_seed)
    seeds = workload_seeds(args.shuffle_seed) if experiment is None else [
        rng.randrange(1, 2**63) for _ in range(experiment['repetitions'])]
    schedule = []
    if experiment is None:
        for repetition, seed in enumerate(seeds, 1):
            cells = list(itertools.product(LAYOUTS, TABLES, backends))
            rng.shuffle(cells)
            for layout, tables, backend in cells:
                schedule.append({'id': 'trial-%03d' % (len(schedule) + 1), 'rep': repetition, 'seed': seed,
                                 'layout': layout, 'tables': tables, 'backend': backend,
                                 'command': command(builds[backend]['binary'], layout, tables, seed)})
    else:
        for repetition, seed in enumerate(seeds, 1):
            cells = list(itertools.product(experiment['workers'], experiment['routing'], backends))
            rng.shuffle(cells)
            for workers, routing, backend in cells:
                schedule.append({'id': 'trial-%03d' % (len(schedule) + 1), 'rep': repetition, 'seed': seed,
                                 'layout': 'split', 'tables': 32, 'workers': workers, 'routing': routing,
                                 'backend': backend, 'diagnostic': False,
                                 'command': command(builds[backend]['binary'], 'split', 32, seed,
                                                    workers=workers, routing=routing)})
        diagnostic_schedule = []
        diagnostic_rng = random.Random(args.shuffle_seed ^ 0x5216fb81)
        for repetition, seed in enumerate(seeds[:3], 1):
            cells = list(itertools.product(experiment['routing'], experiment['diagnostic_backends']))
            diagnostic_rng.shuffle(cells)
            for routing, backend in cells:
                diagnostic_schedule.append({'id': 'diagnostic-%03d' % (len(diagnostic_schedule) + 1),
                    'rep': repetition, 'seed': seed, 'layout': 'split', 'tables': 32,
                    'workers': 32, 'routing': routing, 'backend': backend, 'diagnostic': True,
                    'command': command(builds[backend + '-diagnostic']['binary'], 'split', 32,
                                       seed, workers=32, routing=routing)})
    configuration = {'cpus': CPUS, 'reserved_smt_siblings': list(range(120, 128)),
                     'initial_records': 32768, 'max_inserts_total': 32000000, 'memory_cap_bytes': CAP,
                     'memory_policy': 'bind node1', 'warmup': '8 find-only sweeps of every table by each same live worker'}
    if experiment is not None:
        configuration.update({'cpus': experiment['cpus'], 'workers': experiment['workers'],
            'routing': experiment['routing'], 'max_inserts_per_worker': 4000000,
            'max_inserts_total_by_workers': {str(n): n * 4000000 for n in experiment['workers']},
            'reserved_smt_siblings': [n + 64 for n in experiment['cpus']]})
    manifest = {'schema': SCHEMA if experiment is None else experiment['schema'],
                'experiment': 'actual-upscaledb-multiple-tables-other-locks' if experiment is None else experiment['name'],
                'upstream_pinned_sha': builder.SHA, 'frozen_upstream_root': str(args.frozen_root),
                'rust_sources_sha256': rust_sources_sha256, 'backends': backends, 'excluded_backends': excluded,
                'created_at': utc(), 'output_root': str(root), 'binary_root': str(args.binary_root),
                'expected_trials': len(schedule), 'repetitions': 3 if experiment is None else experiment['repetitions'],
                'seconds': 2, 'shuffle_seed': args.shuffle_seed,
                'workload_seeds': seeds, 'smoke_seconds': .05, 'smoke_invocations': smoke_invocations,
                'timeout_seconds': args.timeout_seconds,
                'machine': receipt, 'configuration': configuration,
                'hypotheses': HYPOTHESES if experiment is None else experiment['hypotheses'],
                'caveats': CAVEATS if experiment is None else experiment['caveats'],
                'builds': builds, 'schedule': schedule,
                **({'diagnostic_schedule': diagnostic_schedule,
                    'expected_diagnostics': len(diagnostic_schedule),
                    'diagnostic_shuffle_seed': args.shuffle_seed ^ 0x5216fb81} if experiment else {}),
                'initial_files': dict(files), 'environment_unset': list(UNSET_ENV)}
    (matrix_backends if experiment is None else experiment['matrix_validator'])(manifest)
    save(root / 'manifest.json', manifest)
    freeze(files, root / 'manifest.json')
    for backend, build in builds.items():
        compile_command = build['command']
        assert_frozen(files)
        record = invoke(root / 'prepare' / (backend + '-compile'), compile_command, args.timeout_seconds)
        require(record['success'], 'compile failed; retained at ' + str(root / 'prepare' / (backend + '-compile')))
        freeze(files, build['binary'])
        dependency_file = root / 'prepare' / (backend + '-dependencies.d')
        dependency_command = list(compile_command)
        dependency_command[dependency_command.index('-o') + 1] = str(dependency_file)
        dependency_command.append('-M')
        record = invoke(root / 'prepare' / (backend + '-dependencies'), dependency_command, args.timeout_seconds)
        require(record['success'], 'dependency extraction failed')
        dependency_text = dependency_file.read_text().replace('\\\n', ' ')
        require(': ' in dependency_text, 'unrecognized compiler dependency output')
        headers = shlex.split(dependency_text.split(': ', 1)[1])
        require(headers, 'empty header dependency closure')
        for header in headers:
            freeze(files, Path(header) if Path(header).is_absolute() else ROOT / header)
        record = invoke(root / 'prepare' / (backend + '-ldd'), [ldd, build['binary']], args.timeout_seconds)
        require(record['success'], 'runtime dependency extraction failed')
        text = (root / 'prepare' / (backend + '-ldd') / 'stdout').read_text()
        require('not found' not in text, 'unresolved runtime library')
        libraries = []
        for line in text.splitlines():
            match = re.search(r'(?:=>\s+)?(/[^\s]+)\s+\(', line)
            if match:
                libraries.append(match.group(1))
                freeze(files, match.group(1))
        require(libraries, 'empty runtime dependency closure')
        save(root / 'prepare' / (backend + '-runtime.json'), libraries)
        if experiment is None:
            for layout, tables in itertools.product(LAYOUTS, TABLES):
                item = {'backend': backend, 'layout': layout, 'tables': tables, 'seed': 1}
                for errors in (False, True):
                    name = '%s-%s-k%d-%s' % (backend, layout, tables, 'errors' if errors else 'smoke')
                    record = invoke(root / 'prepare' / name, command(build['binary'], layout, tables, 1, .05, errors),
                                    args.timeout_seconds,
                                    lambda result, item=item, errors=errors: validate(result, item, .05, errors))
                    require(record['success'], 'smoke failed; retained at ' + str(root / 'prepare' / name))
        else:
            diagnostic = build.get('diagnostic', False)
            gate_backend = build.get('backend', backend)
            worker_counts = (32,) if diagnostic else experiment['workers']
            for workers in worker_counts:
                for routing in experiment['routing']:
                    item = {'backend': gate_backend, 'layout': 'split', 'tables': 32, 'seed': 1,
                            'workers': workers, 'routing': routing, 'diagnostic': diagnostic}
                    name = '%s-w%d-%s-smoke' % (backend, workers, routing)
                    record = invoke(root / 'prepare' / name,
                                    command(build['binary'], 'split', 32, 1, .05, workers=workers, routing=routing),
                                    args.timeout_seconds,
                                    lambda result, item=item: experiment['validator'](result, item, .05))
                    require(record['success'], 'smoke failed; retained at ' + str(root / 'prepare' / name))
                item = {'backend': gate_backend, 'layout': 'split', 'tables': 32, 'seed': 1,
                        'workers': workers, 'routing': 'uniform', 'diagnostic': diagnostic}
                name = '%s-w%d-errors' % (backend, workers)
                record = invoke(root / 'prepare' / name,
                                command(build['binary'], 'split', 32, 1, .05, True, workers, 'uniform'),
                                args.timeout_seconds,
                                lambda result, item=item: experiment['validator'](result, item, .05, True))
                require(record['success'], 'error gate failed; retained at ' + str(root / 'prepare' / name))
    assert_frozen(files)
    require(builder.rust_sources_digest() == rust_sources_sha256, 'Rust source input closure changed during preparation')
    save(root / 'prepared.json', {'schema': SCHEMA if experiment is None else experiment['schema'],
                                 'status': 'prepared', 'completed_at': utc(),
                                 'smoke_invocations': smoke_invocations, 'files': files})


def run(root, experiment=None, diagnostic=False):
    prepared = load(root / 'prepared.json')
    manifest = load(root / 'manifest.json')
    (matrix_backends if experiment is None else experiment['matrix_validator'])(manifest)
    require(not diagnostic or experiment is not None, 'diagnostics require an experiment')
    require(prepared['schema'] == manifest['schema'] and prepared['status'] == 'prepared' and
            prepared['smoke_invocations'] == manifest['smoke_invocations'], 'root is not fully prepared')
    assert_frozen(prepared['files'])
    require(manifest['output_root'] == str(root), 'prepared root cannot be relocated')
    receipt = machine(experiment['cpus'] if experiment else CPUS)
    require(receipt['cpus'] == manifest['machine']['cpus'], 'topology changed')
    directory = root / ('diagnostics' if diagnostic else 'run')
    directory.mkdir()
    save(directory / 'manifest.json', {'schema': manifest['schema'], 'started_at': utc(), 'machine': receipt,
                                      'prepared_sha256': digest(root / 'prepared.json'),
                                      'manifest_sha256': digest(root / 'manifest.json'),
                                      'schedule': manifest['diagnostic_schedule' if diagnostic else 'schedule']})
    status = {'schema': manifest['schema'],
              'expected': manifest['expected_diagnostics' if diagnostic else 'expected_trials'],
              'successful': 0, 'failed': 0, 'attempted': 0, 'records': []}
    try:
        for item in manifest['diagnostic_schedule' if diagnostic else 'schedule']:
            # No compilation, smoke or analysis in run mode; retain every declared condition.
            assert_frozen(prepared['files'])
            record = invoke(directory / item['id'], item['command'], manifest['timeout_seconds'],
                            lambda result, item=item: (validate(result, item) if experiment is None else
                                                       experiment['validator'](result, item)))
            status['attempted'] += 1
            status['successful' if record['success'] else 'failed'] += 1
            status['records'].append({'id': item['id'], 'success': record['success'],
                                      'record_sha256': digest(directory / item['id'] / 'record.json')})
        assert_frozen(prepared['files'])
    except (OSError, ValueError, KeyError, TypeError, KeyboardInterrupt) as exc:
        status['error'] = type(exc).__name__ + ': ' + str(exc)
        raise
    finally:
        # An interrupt can arrive after raw/record persistence but before the normal
        # counter update. Reconcile every planned condition from retained artifacts.
        status['records'] = []
        status['successful'] = status['failed'] = status['attempted'] = 0
        for item in manifest['diagnostic_schedule' if diagnostic else 'schedule']:
            path = directory / item['id'] / 'record.json'
            if path.exists():
                record = load(path)
                status['attempted'] += 1
                status['successful' if record['success'] else 'failed'] += 1
                status['records'].append({'id': item['id'], 'success': record['success'],
                                          'record_sha256': digest(path)})
            elif path.parent.exists():
                status['attempted'] += 1
                status['failed'] += 1
        status['missing'] = status['expected'] - status['attempted']
        status['finished_at'] = utc()
        save(directory / 'status.json', status)
    require(status['failed'] == 0, 'failed trials retained; run --analyze-only for complete accounting')


def quantile(bins, numerator, denominator, minimum):
    total = sum(bins)
    if total < minimum:
        return None
    rank = (total * numerator + denominator - 1) // denominator
    cumulative = 0
    for index, value in enumerate(bins):
        cumulative += value
        if cumulative >= rank:
            return None if index == 63 else (0 if index == 0 else 1 << index)
    return None


def measurements(item, result):
    rows, per_table = [], []
    all_done = sum(t['finds'] + t['inserts'] for w in result['workers'] for t in w['tables'])
    common = {key: item[key] for key in ('rep', 'backend', 'layout', 'tables')}
    common['trial'] = item['id']
    for role_id, role in enumerate(('finds', 'inserts')):
        histograms = [w['latency'][role_id] for w in result['workers']]
        bins = [sum(h['bins'][i] for h in histograms) for i in range(64)]
        samples = sum(bins)
        done = sum(t[role] for w in result['workers'] for t in w['tables'])
        on_time = sum(t[role + '_on_time'] for w in result['workers'] for t in w['tables'])
        rows.append({**common, 'role': role, 'completed': done, 'on_time': on_time, 'late': done - on_time,
                     'throughput_ops_s': on_time / 2, 'request_samples': samples,
                     'request_mean_ns': sum(h['sum_ns'] for h in histograms) / samples if samples else None,
                     'request_p50_upper_ns': quantile(bins, 50, 100, 200),
                     'request_p95_upper_ns': quantile(bins, 95, 100, 2000),
                     'request_p99_upper_ns': quantile(bins, 99, 100, 10000),
                     'request_p999_upper_ns': quantile(bins, 999, 1000, 100000),
                     'request_max_ns': max(h['max_ns'] for h in histograms),
                     'process_cpu_ns_per_completed_op': result['process_cpu_ns'] / all_done if all_done else None,
                     'process_cpu_ns': result['process_cpu_ns'],
                     'worker_cpu_ns': sum(w['cpu_ns'] for w in result['workers']), 'drain_ns': result['drain_ns'],
                     'peak_rss_kib': result['resources']['peak_rss_kib_setup_warmup_measurement'],
                     'final_records': sum(v['seen'] for v in result['verification'])})
        for table in range(item['tables']):
            progress = [w['tables'][table] for w in result['workers']]
            per_table.append({**common, 'role': role, 'table': table,
                              'completed': sum(t[role] for t in progress),
                              'on_time': sum(t[role + '_on_time'] for t in progress),
                              'throughput_ops_s': sum(t[role + '_on_time'] for t in progress) / 2})
    return rows, per_table


def describe(points):
    return {'points': points, 'available_n': len(points), 'median': statistics.median(points) if points else None,
            'observed_min': min(points) if points else None, 'observed_max': max(points) if points else None}


def screen(points):
    if len(points) != 3:
        return 'incomplete'
    if all(value > 1.05 for value in points):
        return 'consistent >5% increase'
    if all(value < .95 for value in points):
        return 'consistent >5% decrease'
    return 'mixed/small (not equivalence)'


def comparisons(rows, backends):
    result = []
    metrics = ('throughput_ops_s', 'request_mean_ns', 'request_p99_upper_ns', 'process_cpu_ns_per_completed_op')
    lookup = {(r['layout'], r['tables'], r['backend'], r['role'], r['rep']): r for r in rows}
    pairs = list(itertools.permutations(backends, 2))
    for layout, tables, role, (backend, reference), metric in itertools.product(LAYOUTS, TABLES, ('finds', 'inserts'), pairs, metrics):
        points = []
        reps = []
        for rep in range(1, 4):
            a = lookup.get((layout, tables, backend, role, rep))
            b = lookup.get((layout, tables, reference, role, rep))
            if a and b and a[metric] is not None and b[metric] is not None and a[metric] > 0 and b[metric] > 0:
                points.append(a[metric] / b[metric]); reps.append(rep)
        result.append({'contrast': 'backend/reference', 'layout': layout, 'tables': tables, 'role': role,
                       'backend': backend, 'reference': reference, 'metric': metric, 'paired_repetitions': reps,
                       'orientation': 'higher is better' if metric == 'throughput_ops_s' else 'lower is better',
                       'ratio': describe(points), 'screen': screen(points)})
    for tables, backend, role, metric in itertools.product(TABLES, backends, ('finds', 'inserts'), metrics):
        points, reps = [], []
        for rep in range(1, 4):
            a = lookup.get(('split', tables, backend, role, rep))
            b = lookup.get(('coarse', tables, backend, role, rep))
            if a and b and a[metric] is not None and b[metric] is not None and a[metric] > 0 and b[metric] > 0:
                points.append(a[metric] / b[metric]); reps.append(rep)
        result.append({'contrast': 'split/coarse', 'tables': tables, 'role': role, 'backend': backend,
                       'metric': metric, 'paired_repetitions': reps,
                       'orientation': 'higher is better' if metric == 'throughput_ops_s' else 'lower is better',
                       'ratio': describe(points), 'screen': screen(points)})
    return result


def write_csv(path, rows, columns):
    with path.open('x', newline='') as target:
        writer = csv.DictWriter(target, fieldnames=columns)
        writer.writeheader()
        writer.writerows(rows)


def figures(directory, rows, failed, missing, backends, excluded):
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    from matplotlib.ticker import FuncFormatter, SymmetricalLogLocator
    metrics = [('throughput_ops_s', 'On-time operations/s'),
               ('request_mean_ns', 'Mean requester API latency (ns)'),
               ('request_p99_upper_ns', 'P99 requester latency upper bin (ns)'),
               ('process_cpu_ns_per_completed_op', 'Process CPU ns / all completed operations')]
    markers = ('o', 's', '^', 'D', 'v', 'P', 'X', '<', '>', '*')
    for metric, label in metrics:
        fig, axes = plt.subplots(2, 2, figsize=(15, 10), squeeze=False)
        for row, layout in enumerate(LAYOUTS):
            for col, role in enumerate(('finds', 'inserts')):
                axis = axes[row][col]
                unavailable = 0
                for backend_index, backend in enumerate(backends):
                    style_index = BACKENDS.index(backend)
                    color = BACKEND_COLORS[style_index]
                    offset = (backend_index - (len(backends) - 1) / 2) * .032
                    x = [index + offset for index in range(len(TABLES))]
                    medians = []
                    for index, tables in enumerate(TABLES):
                        points = [r[metric] for r in rows if (r['layout'], r['tables'], r['backend'], r['role']) ==
                                  (layout, tables, backend, role) and r[metric] is not None]
                        medians.append(statistics.median(points) if points else math.nan)
                        unavailable += not points
                        axis.scatter([x[index]] * len(points), points, s=13, alpha=.5, color=color)
                    axis.plot(x, medians, marker=markers[style_index], markersize=4,
                              linewidth=1.1, label=BACKEND_LABELS[backend], color=color,
                              linestyle='--' if style_index >= len(CONTROL_BACKENDS) else '-')
                axis.set_xticks(range(len(TABLES)), ['1 (control)', '8', '32'])
                axis.set_xlim(-.3, len(TABLES) - .7)
                axis.set_yscale('symlog', linthresh=1)
                values = [r[metric] for r in rows if r['layout'] == layout and r['role'] == role
                          and r[metric] is not None]
                if values:
                    axis.set_ylim(max(0, min(values) * .75), max(1, max(values) * 1.25))
                axis.yaxis.set_major_locator(SymmetricalLogLocator(base=10, linthresh=1, subs=(1, 2, 5)))
                axis.yaxis.set_major_formatter(FuncFormatter(lambda value, _: f'{value:g}'))
                axis.set_xlabel('Tables K'); axis.set_ylabel(label)
                axis.set_title(layout + ' / ' + role)
                axis.grid(alpha=.2, which='both')
                if unavailable:
                    axis.text(.02, .03, '%d backend/K cells unavailable' % unavailable,
                              transform=axis.transAxes, fontsize=8)
        handles, labels = axes[0][0].get_legend_handles_labels()
        fig.legend(handles, labels, loc='lower center', ncol=5, fontsize=9, frameon=False,
                   bbox_to_anchor=(.5, .01))
        fig.suptitle('Actual UpScaleDB tables; n=3 exploratory; %d failed, %d missing; excluded: %s\n'
                     'Points + medians, no CIs; y logarithmic above 1; missing p99 is not zero' %
                     (failed, missing, ', '.join(excluded) or 'none'), fontsize=12)
        fig.tight_layout(rect=(0, .09, 1, .94))
        fig.savefig(directory / (metric + '.png'), dpi=150)
        fig.savefig(directory / (metric + '.svg'))
        plt.close(fig)
    return {'matplotlib_version': matplotlib.__version__, 'figures': [m[0] for m in metrics],
            'backends': list(backends), 'excluded_backends': excluded,
            'y_scale': 'symlog, linear threshold 1; null quantiles absent', 'k1': 'implementation control'}


def analyze(root):
    manifest = load(root / 'manifest.json')
    backends = matrix_backends(manifest)
    expected = manifest['expected_trials']
    prepared = load(root / 'prepared.json')
    require(digest(root / 'manifest.json') == prepared['files'][str(root / 'manifest.json')], 'manifest changed after preparation')
    run_manifest = load(root / 'run/manifest.json') if (root / 'run/manifest.json').exists() else None
    if run_manifest:
        require(run_manifest['prepared_sha256'] == digest(root / 'prepared.json') and
                run_manifest['manifest_sha256'] == digest(root / 'manifest.json'), 'run provenance mismatch')
    directory = root / 'analysis'
    directory.mkdir()
    trials, rows, per_table = [], [], []
    status_path = root / 'run/status.json'
    recorded_hashes = {r['id']: r['record_sha256'] for r in load(status_path)['records']} if status_path.exists() else {}
    for item in manifest['schedule']:
        path = root / 'run' / item['id'] / 'record.json'
        trial = {'item': item, 'status': 'missing', 'record_path': str(path)}
        if path.exists():
            try:
                trial['record_sha256'] = digest(path)
                if item['id'] in recorded_hashes:
                    require(trial['record_sha256'] == recorded_hashes[item['id']], 'record hash mismatch')
                record = load(path)
                trial['record'] = record
                require(record['command'] == item['command'], 'record command mismatch')
                for name in ('stdout', 'stderr', 'invocation.json'):
                    require(digest(path.parent / name) == record[name + '_sha256'], name + ' hash mismatch')
                require(record['success'], record.get('error', 'recorded process failure'))
                require(load(path.parent / 'stdout') == record['result'], 'raw result differs from recorded result')
                validate(record['result'], item)
                trial['status'] = 'ok'
                new_rows, new_tables = measurements(item, record['result'])
                rows.extend(new_rows); per_table.extend(new_tables)
            except (OSError, ValueError, KeyError, TypeError) as exc:
                trial['status'] = 'failed'; trial['error'] = str(exc)
        elif path.parent.exists():
            trial['status'] = 'failed'; trial['error'] = 'partial invocation; inspect retained raw files'
        trials.append(trial)
    failed = sum(t['status'] == 'failed' for t in trials)
    missing = sum(t['status'] == 'missing' for t in trials)
    metrics = ('throughput_ops_s', 'request_mean_ns', 'request_p50_upper_ns', 'request_p95_upper_ns',
               'request_p99_upper_ns', 'request_p999_upper_ns', 'request_max_ns',
               'process_cpu_ns_per_completed_op', 'drain_ns', 'peak_rss_kib', 'final_records')
    summaries = []
    for layout, tables, backend, role in itertools.product(LAYOUTS, TABLES, backends, ('finds', 'inserts')):
        selected = [r for r in rows if (r['layout'], r['tables'], r['backend'], r['role']) == (layout, tables, backend, role)]
        cell = [t for t in trials if (t['item']['layout'], t['item']['tables'], t['item']['backend']) == (layout, tables, backend)]
        summaries.append({'layout': layout, 'tables': tables, 'backend': backend, 'role': role,
                          'expected_n': 3, 'successful_n': len(selected),
                          'failed_n': sum(t['status'] == 'failed' for t in cell), 'missing_n': sum(t['status'] == 'missing' for t in cell),
                          'metrics': {m: describe([r[m] for r in selected if r[m] is not None]) for m in metrics}})
    contrasts = comparisons(rows, backends)
    k1_controls = [c for c in contrasts if c['contrast'] == 'split/coarse' and c['tables'] == 1]
    summary = {'schema': SCHEMA, 'analyzed_at': utc(), 'expected_trials': expected,
               'successful_trials': expected - failed - missing, 'backends': backends,
               'excluded_backends': manifest['excluded_backends'], 'workload_seeds': manifest['workload_seeds'],
               'rust_sources_sha256': manifest['rust_sources_sha256'], 'k1_controls': k1_controls,
               'failed_trials': failed, 'missing_trials': missing, 'complete': failed == missing == 0,
               'manifest_sha256': digest(root / 'manifest.json'), 'prepared_sha256': digest(root / 'prepared.json'),
               'hypotheses': manifest['hypotheses'], 'caveats': manifest['caveats'], 'trials': trials,
               'measurements': rows, 'per_table_progress': per_table, 'summaries': summaries, 'contrasts': contrasts}
    save(directory / 'summary.json', summary)
    write_csv(directory / 'trials.csv', [{'trial': t['item']['id'], **{k: t['item'][k] for k in ('rep', 'layout', 'tables', 'backend')},
              'status': t['status'], 'error': t.get('error', t.get('record', {}).get('error', ''))} for t in trials],
              ['trial', 'rep', 'layout', 'tables', 'backend', 'status', 'error'])
    columns = ['rep', 'backend', 'layout', 'tables', 'trial', 'role', 'completed', 'on_time', 'late', 'throughput_ops_s',
               'request_samples', 'request_mean_ns', 'request_p50_upper_ns', 'request_p95_upper_ns', 'request_p99_upper_ns',
               'request_p999_upper_ns', 'request_max_ns', 'process_cpu_ns_per_completed_op', 'process_cpu_ns', 'worker_cpu_ns',
               'drain_ns', 'peak_rss_kib', 'final_records']
    write_csv(directory / 'measurements.csv', rows, columns)
    write_csv(directory / 'tables.csv', per_table,
              ['rep', 'backend', 'layout', 'tables', 'trial', 'role', 'table', 'completed', 'on_time', 'throughput_ops_s'])
    contrast_columns = ['contrast', 'layout', 'tables', 'backend', 'reference', 'role', 'metric', 'orientation',
                        'paired_repetitions', 'available_n', 'median', 'observed_min', 'observed_max', 'points', 'screen']
    contrast_rows = []
    for contrast in contrasts:
        row = {key: contrast.get(key, '') for key in contrast_columns}
        row.update(contrast['ratio'])
        row['points'] = json.dumps(row['points'])
        row['paired_repetitions'] = json.dumps(row['paired_repetitions'])
        contrast_rows.append(row)
    write_csv(directory / 'contrasts.csv', contrast_rows, contrast_columns)
    lines = ['Actual UpScaleDB multi-table other-locks boundary (frozen upstream DB; current Rust bridge)',
             '%d/%d successful; %d failed; %d missing.' % (expected - failed - missing, expected, failed, missing),
             'Predeclared backends: ' + ', '.join(backends),
             'Rust source closure SHA256: ' + manifest['rust_sources_sha256'],
             'Paired workload seeds: ' + ', '.join(map(str, manifest['workload_seeds'])),
             '', 'Predeclared safety/build exclusions:',
             *['- %s: %s' % (backend, reason) for backend, reason in manifest['excluded_backends'].items()],
             '', 'Hypotheses (not conclusions):', *['- ' + h for h in manifest['hypotheses']],
             '', 'Scope/caveats:', *['- ' + c for c in manifest['caveats']],
             '', 'All declared conditions retained below; failed/missing trials are not favorable-result exclusions.']
    for cell in summaries:
        rate = cell['metrics']['throughput_ops_s']
        latency = cell['metrics']['request_mean_ns']
        p99 = cell['metrics']['request_p99_upper_ns']
        lines.append('%s K=%d %s %s: n=%d/3 failed=%d missing=%d; ops/s median=%s range=[%s,%s]; mean request ns median=%s range=[%s,%s]' %
                     (cell['layout'], cell['tables'], cell['backend'], cell['role'], cell['successful_n'], cell['failed_n'], cell['missing_n'],
                      rate['median'], rate['observed_min'], rate['observed_max'], latency['median'], latency['observed_min'], latency['observed_max']))
        lines.append('  p99 upper-bin ns: available=%d/3 median=%s range=[%s,%s]; unavailable is not zero.' %
                     (p99['available_n'], p99['median'], p99['observed_min'], p99['observed_max']))
    lines.extend(['', 'K=1 implementation controls (split/coarse):',
                  'Both layouts have one environment and one gate. Differences below are variation, not a granularity benefit.'])
    for contrast in k1_controls:
        lines.append('%s %s %s: %s points=%s' % (contrast['backend'], contrast['role'], contrast['metric'],
                                                contrast['screen'], contrast['ratio']['points']))
    lines.extend(['', 'Paired descriptive screen: all three ratios >1.05=increase, all <0.95=decrease, otherwise mixed/small.',
                  'Ratios are numerator/reference, higher better only for throughput; lower better for latency/CPU. Not significance or equivalence.',
                  'All ordered backend pairs are in summary.json and contrasts.csv; selected contrasts follow.'])
    for contrast in contrasts:
        pair = (contrast.get('backend'), contrast.get('reference'))
        if contrast['contrast'] == 'split/coarse' or pair == ('fc_pq', 'fc') or pair[0] == 'uscl':
            lines.append('%s %s K=%d backend=%s reference=%s %s %s: %s points=%s' %
                         (contrast['contrast'], contrast.get('layout', ''), contrast['tables'], contrast['backend'],
                          contrast.get('reference', '(same backend)'), contrast['role'], contrast['metric'],
                          contrast['screen'], contrast['ratio']['points']))
    with (directory / 'report.txt').open('x') as target:
        target.write('\n'.join(lines) + '\n')
    plot_record = figures(directory, rows, failed, missing, backends, manifest['excluded_backends'])
    save(directory / 'figures.json', plot_record)
    return directory / 'summary.json'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-root', type=Path, required=True)
    parser.add_argument('--binary-root', type=Path, default=BUILD_ROOT)
    parser.add_argument('--frozen-root', type=Path, default=FROZEN_ROOT,
                        help='verified native/FC baseline build root (defaults to unified build root)')
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--prepare-only', action='store_true', help='fresh root: frozen compile + all layout/K smoke and real status gates')
    mode.add_argument('--run', action='store_true', help='predeclared primary timing matrix only; prepared root required')
    mode.add_argument('--analyze-only', action='store_true', help='saved results only; fresh analysis directory required')
    parser.add_argument('--shuffle-seed', type=int, default=2026092456)
    parser.add_argument('--timeout-seconds', type=float, default=180, help='hard per-process watchdog, including setup/oracle')
    parser.add_argument('--backends', nargs='+', choices=BACKENDS, default=BACKENDS,
                        help='prepare only: explicit backend set, default all safety-admitted candidates')
    parser.add_argument('--exclusion-reason', help='prepare only: evidenced reason for omitting any additional backend')
    args = parser.parse_args()
    args.output_root = args.output_root.expanduser().resolve()
    args.binary_root = args.binary_root.expanduser().resolve()
    args.frozen_root = args.frozen_root.expanduser().resolve()
    require(args.prepare_only or (tuple(args.backends) == BACKENDS and args.exclusion_reason is None),
            '--backends/--exclusion-reason apply only to preparation; saved matrix is immutable')
    require(math.isfinite(args.timeout_seconds) and 1 <= args.timeout_seconds <= 600, 'watchdog must be in [1,600] seconds')
    signal.signal(signal.SIGINT, interrupt); signal.signal(signal.SIGTERM, interrupt)
    if args.prepare_only:
        prepare(args); print(args.output_root / 'prepared.json')
    elif args.run:
        run(args.output_root); print(args.output_root / 'run/status.json')
    else:
        print(analyze(args.output_root))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, KeyboardInterrupt) as exc:
        sys.exit(type(exc).__name__ + ': ' + str(exc))
