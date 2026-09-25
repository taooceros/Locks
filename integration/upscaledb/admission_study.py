#!/usr/bin/env python3
"""Historical counted-vs-external admission comparison (requires the old admission ABI)."""
import argparse
import csv
import io
import json
import math
from pathlib import Path
import random
import shutil
import signal
import statistics
import sys
import subprocess
import tarfile

from analyze import validate
from run_trials import (HERE, ROOT, TrialStop, atomic_new, build_provenance,
                        capture_sources, check_memory_selection, digest,
                        discover_topology, environment, parse_cpu_ranges, read_text,
                        reject_nonfinite, request_stop, run_trial, source_snapshot,
                        trial_command)
from scaling import select_cpus

BACKENDS = ('bridge_mutex', 'fc', 'fc_pq', 'uscl', 'cfl_local')
ADMISSIONS = ('counted', 'external')
METRICS = ('elapsed_s', 'total_ops_s', 'find_latency_mean_ns',
           'insert_latency_mean_ns', 'timed_process_cpu_s')
DEFAULT_BINARY = ROOT / '.worktree' / 'upscaledb-admission-build'
DEFAULT_OUTPUT = ROOT / '.worktree' / 'upscaledb-admission-study'


def placements(topology):
    sockets = sorted({(cpu['socket'], cpu['node']) for cpu in topology['cpus'].values()})
    if (0, 0) not in sockets or len(sockets) != 2 or sockets[1][0] == 0:
        raise ValueError('admission study requires node0/socket0 and one other socket')
    definitions = (('single', 1, 'compact', False, 0),
                   ('same_socket', 8, 'compact', False, 0),
                   ('cross_socket', 8, 'balanced', False, None),
                   ('smt', 4, 'compact', True, 0))
    result = []
    init_candidates = [int(cpu) for cpu, info in topology['cpus'].items() if info['node'] == 0]
    if not init_candidates:
        raise ValueError('no allowed initialization CPU on node 0')
    for name, cores, placement, smt, socket in definitions:
        cpus = select_cpus(topology, cores, placement, smt=smt, socket=socket)
        workers = len(cpus)
        if name == 'cross_socket':
            roles = [cpus[:4], cpus[4:]]
            if any({s: sum(topology['cpus'][str(cpu)]['socket'] == s for cpu in role)
                    for s, _ in sockets} != {s: 2 for s, _ in sockets} for role in roles):
                raise ValueError('cross-socket finder/inserter roles must each balance sockets')
        result.append({'name': name, 'cpus': cpus, 'workers': workers,
                       'physical_cores': cores, 'roles': [1, 0] if workers == 1 else [4, 4],
                       'init_cpu': cpus[0] if topology['cpus'][str(cpus[0])]['node'] == 0
                       else min(init_candidates), 'init_node': 0,
                       'memory_policy': 'bind', 'memory_nodes': [0],
                       'smt': smt, 'placement': placement})
    return result


def make_schedule(cases, backends, repetitions, seed):
    rng = random.Random(seed)
    schedule = []
    for block in range(repetitions):
        # Shuffle all 40 conditions, not just the two lifecycles in a fixed order.
        conditions = [(case['name'], backend, admission)
                      for case in cases for backend in backends for admission in ADMISSIONS]
        rng.shuffle(conditions)
        for position, (placement, backend, admission) in enumerate(conditions):
            schedule.append({'block': block, 'position': position, 'placement': placement,
                             'variant': backend, 'admission': admission,
                             'file': f'block-{block:04d}.position-{position:02d}.json'})
    return schedule


def config_for(case, options):
    return {'variants': list(options.backends), 'mode': 'fixed',
            'finders': case['roles'][0], 'inserters': case['roles'][1],
            'workers': case['workers'], 'cpus': case['cpus'],
            'layout': 'split' if case['placement'] == 'balanced' else 'packed',
            'repetitions': options.repetitions, 'seconds': 10.0, 'warmup': 0.0,
            'seed': 1, 'order_seed': options.order_seed, 'reads': options.reads,
            'inserts': options.inserts, 'preload': options.preload,
            'max_inserts': 200000000, 'memory_limit_gib': 8,
            'wait_proxy_ns': 1000, 'timeout_seconds': options.timeout_seconds,
            'smoke': options.smoke, 'exclusive_cpus': True,
            'init_cpu': case['init_cpu'], 'init_node': 0,
            'requested_memory_policy': 'bind', 'memory_nodes': [0]}


def planned_manifest(options, topology):
    cases = placements(topology)
    schedule = make_schedule(cases, options.backends, options.repetitions, options.order_seed)
    configs = {case['name']: config_for(case, options) for case in cases}
    commands = {}
    for item in schedule:
        cfg = configs[item['placement']]
        args = argparse.Namespace(mode='fixed', cpus=cfg['cpus'], init_cpu=cfg['init_cpu'],
                                  init_node=0, roles=(cfg['finders'], cfg['inserters']),
                                  preload=cfg['preload'], reads=cfg['reads'], inserts=cfg['inserts'],
                                  max_inserts=cfg['max_inserts'], memory_limit_gib=cfg['memory_limit_gib'],
                                  seed=1, wait_proxy_ns=cfg['wait_proxy_ns'], seconds=cfg['seconds'],
                                  warmup=0.0, memory_policy='bind', memory_nodes=[0])
        binary = options.binary_root / ('upscaledb-' + item['variant'])
        commands[item['file']] = trial_command(binary, args) + ['--admission', item['admission']]
    return {'schema': 1, 'kind': 'admission_cost_smoke' if options.smoke else 'admission_cost',
            'binary_root': str(options.binary_root), 'output_root': str(options.output_root),
            'topology': topology,
            'placements': cases, 'configs': configs, 'schedule': schedule, 'commands': commands,
            'backends': list(options.backends), 'repetitions': options.repetitions,
            'order_seed': options.order_seed, 'smoke': options.smoke,
            'timeout_seconds': options.timeout_seconds}


def source_files(provenance):
    files = {str(ROOT / relative): value for relative, value in
             provenance['integration_sources']['files'].items()}
    files.update({str(HERE / name): digest(HERE / name) for name in
                  ('admission_study.py', 'test_admission_study.py')})
    files[str(HERE / 'bridge-ops.patch')] = digest(HERE / 'bridge-ops.patch')
    # Build provenance checks Rust's aggregate digest; keep individual inputs for replay.
    rust = [ROOT / 'Cargo.toml', ROOT / 'Cargo.lock', HERE / 'bridge.h']
    rust.extend((ROOT / '.cargo').glob('*.toml'))
    for suffix in ('*.c', '*.h'):
        rust.extend((ROOT / 'c').rglob(suffix))
    for crate in ('libdlock', 'upscaledb-bridge'):
        directory = ROOT / 'crates' / crate
        rust.append(directory / 'Cargo.toml')
        rust.extend(directory.glob('src/**/*.rs'))
        rust.extend(directory.glob('binding/**/*.h'))
        if (directory / 'build.rs').is_file():
            rust.append(directory / 'build.rs')
    files.update({str(path): digest(path) for path in rust})
    for artifact in provenance['binaries'].values():
        build = artifact['build_manifest']
        files[str(Path(build['source']) / 'src/5upscaledb/upscaledb.cc')] = (
            build['source_verification']['implementation_sha256'])
        files[str(Path(build['source']) / 'configure')] = build['hashes']['configure_sha256']
        files[str(Path(build['source']) / 'config.h.in')] = build['hashes']['config_h_in_sha256']
    if any(value is None for value in files.values()):
        raise ValueError('missing source input')
    return files


def frozen_files(provenance):
    files = {}
    for artifact in provenance['binaries'].values():
        build = artifact['build_manifest']
        files[artifact['path']] = artifact['sha256']
        files[artifact['build_manifest_path']] = artifact['build_manifest_sha256']
        library = build['library_verification']
        files[library['shared_object']] = library['shared_object_sha256']
        rust = build['rust_staticlib']
        if rust is None:
            raise ValueError('bridge build missing Rust archive')
        files[rust['archive']] = rust['archive_sha256']
    return files


def assert_frozen(manifest):
    for name, expected in {**manifest['source_files'], **manifest['frozen_files']}.items():
        if digest(Path(name)) != expected:
            raise ValueError('frozen input changed or disappeared: ' + name)
    for backend, artifact in manifest['artifacts']['binaries'].items():
        path = Path(artifact['path'])
        current = build_provenance(backend, path, Path(artifact['build_manifest_path']))
        if current['sha256'] != artifact['sha256'] or current['build_manifest_sha256'] != artifact['build_manifest_sha256']:
            raise ValueError(backend + ': executable/build provenance changed')


def archive_files(output, files):
    """Copy immutable inputs before launching; archived copies can be audited offline."""
    target = output / 'frozen'
    target.mkdir()
    copied = {}
    for index, (source, expected) in enumerate(sorted(files.items())):
        if not expected or digest(Path(source)) != expected:
            raise ValueError('source changed before archival: ' + source)
        destination = target / f'{index:04d}'
        with Path(source).open('rb') as reader, destination.open('xb') as writer:
            shutil.copyfileobj(reader, writer, 1024 * 1024)
        if digest(destination) != expected:
            raise ValueError('archived input differs: ' + source)
        copied[source] = {'file': str(destination.relative_to(output)), 'sha256': expected}
    return copied


def archive_study_sources(output):
    target = output / 'study-sources.tar.gz'
    with tarfile.open(target, 'x:gz') as archive:
        for name in ('admission_study.py', 'test_admission_study.py'):
            data = (HERE / name).read_bytes()
            info = tarfile.TarInfo('integration/upscaledb/' + name)
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
    return {'file': target.name, 'sha256': digest(target)}


def record_valid(record, item, manifest, baseline_flags):
    filename = item['file']
    cfg = manifest['configs'][item['placement']]
    command = manifest['commands'][filename]
    variant = item['variant']
    if (record.get('schema') != 1 or record.get('item') != item or
            record.get('variant') != variant or record.get('block') != item['block'] or
            record.get('config') != cfg or record.get('artifacts') != manifest['artifacts'] or
            record.get('machine') != manifest['machine'] or record.get('command') != command or
            record.get('binary_sha256_before') != manifest['artifacts']['binaries'][variant]['sha256'] or
            record.get('binary_sha256_after') != manifest['artifacts']['binaries'][variant]['sha256'] or
            record.get('artifact_error') or record.get('freeze_error')):
        raise ValueError(filename + ': raw record/provenance/command mismatch')
    if (command[0] != manifest['artifacts']['binaries'][variant]['path'] or
            command.count('--admission') != 1 or
            command[command.index('--admission') + 1:] != [item['admission']] or
            record.get('admission') != item['admission'] or
            not isinstance(record.get('result'), dict) or
            record['result'].get('bridge_admission') != item['admission']):
        raise ValueError(filename + ': lifecycle label/command mismatch')
    checked = validate(record,
                       {'config': cfg, 'artifacts': manifest['artifacts']}, baseline_flags)
    if 'timed_process_cpu_s' not in checked['metrics']:
        raise ValueError(filename + ': timed process CPU unavailable')
    return checked


def summarize_trials(trials, schedule, backends, cases, repetitions):
    """Require the full matched matrix; speedup >1 always favors external."""
    expected = {(b, c['name'], v, a) for b in range(repetitions)
                for c in cases for v in backends for a in ADMISSIONS}
    observed = {}
    for entry in trials:
        key = (entry['block'], entry['placement'], entry['variant'], entry['admission'])
        if key in observed:
            raise ValueError('duplicate matched condition: ' + str(key))
        observed[key] = entry
    if set(observed) != expected or len(schedule) != len(expected):
        raise ValueError('missing or extra matched conditions: ' + str(sorted(expected ^ set(observed))))
    directions = {'elapsed_s': ('counted', 'external'),
                  'total_ops_s': ('external', 'counted'),
                  'find_latency_mean_ns': ('counted', 'external'),
                  'insert_latency_mean_ns': ('counted', 'external'),
                  'timed_process_cpu_s': ('counted', 'external')}
    rows = []
    summary = []
    for case in cases:
        for backend in backends:
            pairs = []
            for block in range(repetitions):
                counted = observed[(block, case['name'], backend, 'counted')]['metrics']
                external = observed[(block, case['name'], backend, 'external')]['metrics']
                ratios = {}
                for metric, (numerator, denominator) in directions.items():
                    values = {'counted': counted[metric], 'external': external[metric]}
                    if not all(isinstance(v, (int, float)) and math.isfinite(v) and v > 0
                               for v in values.values()):
                        raise ValueError('nonpositive/nonfinite metric ' + metric)
                    ratios[metric] = values[numerator] / values[denominator]
                rows.append({'block': block, 'placement': case['name'], 'variant': backend,
                             'counted': counted, 'external': external, 'speedup_external': ratios})
                pairs.append(rows[-1])
            summary.append({'placement': case['name'], 'variant': backend, 'n_pairs': len(pairs),
                            'speedup_external': {metric: {'ratios': [p['speedup_external'][metric] for p in pairs],
                                                         'mean': statistics.mean(p['speedup_external'][metric] for p in pairs),
                                                         'median': statistics.median(p['speedup_external'][metric] for p in pairs),
                                                         'min': min(p['speedup_external'][metric] for p in pairs),
                                                         'max': max(p['speedup_external'][metric] for p in pairs)}
                                                 for metric in METRICS},
                            'levels': {mode: {metric: {'mean': statistics.mean(p[mode][metric] for p in pairs),
                                                        'min': min(p[mode][metric] for p in pairs),
                                                        'max': max(p[mode][metric] for p in pairs)}
                                              for metric in METRICS} for mode in ADMISSIONS}})
    return rows, summary


def analyze_saved(root, analysis_dir):
    manifest = json.loads((root / 'manifest.json').read_text(), parse_constant=reject_nonfinite)
    if manifest.get('schema') != 1 or manifest.get('kind') not in ('admission_cost', 'admission_cost_smoke'):
        raise ValueError('unsupported admission study manifest')
    cases = manifest['placements']
    schedule = manifest['schedule']
    conditions = {(case['name'], backend, admission) for case in cases
                  for backend in manifest['backends'] for admission in ADMISSIONS}
    positions = {}
    for item in schedule:
        block, position = item['block'], item['position']
        if (not isinstance(block, int) or not 0 <= block < manifest['repetitions'] or
                not isinstance(position, int) or
                item['file'] != f'block-{block:04d}.position-{position:02d}.json' or
                (item['placement'], item['variant'], item['admission']) not in conditions):
            raise ValueError('invalid recorded schedule condition')
        positions.setdefault(block, []).append(item)
    if (len(cases) != len({case['name'] for case in cases}) or
            set(positions) != set(range(manifest['repetitions'])) or
            any(len(items) != len(conditions) or
                [item['position'] for item in items] != list(range(len(conditions))) or
                {(item['placement'], item['variant'], item['admission']) for item in items} != conditions
                for items in positions.values()) or
            len(set(manifest['backends'])) != len(manifest['backends']) or
            any(b not in BACKENDS for b in manifest['backends']) or
            manifest['smoke'] != (manifest['kind'] == 'admission_cost_smoke') or
            set(manifest['configs']) != {case['name'] for case in cases} or
            set(manifest['commands']) != {item['file'] for item in schedule}):
        raise ValueError('incomplete/invalid frozen schedule or configuration')
    if {case['name'] for case in cases} != {'single', 'same_socket', 'cross_socket', 'smt'}:
        raise ValueError('missing admission topology placement')
    if manifest['smoke']:
        if (not 1 <= manifest['repetitions'] <= 2 or
                any(cfg['reads'] + cfg['inserts'] > 10000 or cfg['preload'] > 10000
                    for cfg in manifest['configs'].values())):
            raise ValueError('oversized trial falsely labeled smoke')
    elif (manifest['repetitions'] != 5 or manifest['backends'] != list(BACKENDS) or
          any((cfg['reads'], cfg['inserts'], cfg['preload']) != (400000, 400000, 100000)
              for cfg in manifest['configs'].values())):
        raise ValueError('incomplete primary admission workload')
    for case in cases:
        cfg = manifest['configs'][case['name']]
        if (cfg['mode'] != 'fixed' or cfg['warmup'] != 0 or cfg['seed'] != 1 or
                cfg['cpus'] != case['cpus'] or cfg['workers'] != case['workers'] or
                cfg['finders'] != case['roles'][0] or cfg['inserters'] != case['roles'][1] or
                cfg['requested_memory_policy'] != 'bind' or cfg['memory_nodes'] != [0] or
                cfg['init_node'] != 0 or
                manifest['topology']['cpus'][str(cfg['init_cpu'])]['node'] != 0):
            raise ValueError('saved study configuration violates controlled workload')
    actual = {p.name for p in root.glob('*.json')}
    expected = {'manifest.json'} | {item['file'] for item in schedule}
    if actual != expected:
        raise ValueError('missing/extra raw records: ' + str(sorted(actual ^ expected)))
    for stored in (manifest['integration_sources'], manifest['source_snapshot'],
                   manifest['study_sources']):
        if digest(root / stored['file']) != stored['sha256']:
            raise ValueError('missing/changed source snapshot: ' + stored['file'])
    for source, details in manifest['frozen_archive'].items():
        if digest(root / details['file']) != details['sha256'] or details['sha256'] != (
                manifest['source_files'] | manifest['frozen_files'])[source]:
            raise ValueError('missing/changed frozen input: ' + source)
    if set(manifest['frozen_archive']) != set(manifest['source_files']) | set(manifest['frozen_files']):
        raise ValueError('frozen input inventory incomplete')
    entries = []
    baseline = None
    for item in schedule:
        record = json.loads((root / item['file']).read_text(), parse_constant=reject_nonfinite)
        checked = record_valid(record, item, manifest, baseline)
        baseline = checked['flags']
        entries.append({**item, 'metrics': {key: checked['metrics'][key] for key in METRICS}})
    rows, groups = summarize_trials(entries, schedule, manifest['backends'], cases, manifest['repetitions'])
    if analysis_dir.exists():
        raise ValueError('refusing existing analysis directory: ' + str(analysis_dir))
    analysis_dir.mkdir(parents=True)
    atomic_new(analysis_dir / 'summary.json', {'schema': 1, 'source_manifest': str(root / 'manifest.json'),
               'source_manifest_sha256': digest(root / 'manifest.json'),
               'raw_sha256': {item['file']: digest(root / item['file']) for item in schedule},
               'classification': 'smoke_not_primary' if manifest['smoke'] else 'controlled_cold_start',
               'n_trials': len(entries), 'n_pairs': len(rows), 'metrics': list(METRICS),
               'interpretation': f"Ratios >1 favor external; {manifest['repetitions']} observed paired ratios/ranges, not confidence intervals. Bridge-mutex aggregate is not admission-only cost.",
               'groups': groups, 'trials': entries})
    with (analysis_dir / 'trials.csv').open('x', newline='') as output:
        writer = csv.DictWriter(output, fieldnames=['block', 'placement', 'variant', 'admission', 'raw_file', *METRICS])
        writer.writeheader()
        for entry in entries:
            writer.writerow({'block': entry['block'], 'placement': entry['placement'],
                             'variant': entry['variant'], 'admission': entry['admission'],
                             'raw_file': entry['file'], **entry['metrics']})
    with (analysis_dir / 'paired.csv').open('x', newline='') as output:
        columns = ['block', 'placement', 'variant'] + [f'{metric}_{mode}' for metric in METRICS
                                                   for mode in ADMISSIONS] + [f'{metric}_speedup_external' for metric in METRICS]
        writer = csv.DictWriter(output, fieldnames=columns)
        writer.writeheader()
        for row in rows:
            writer.writerow({'block': row['block'], 'placement': row['placement'], 'variant': row['variant'],
                             **{f'{metric}_{mode}': row[mode][metric] for metric in METRICS for mode in ADMISSIONS},
                             **{f'{metric}_speedup_external': row['speedup_external'][metric] for metric in METRICS}})
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    fig, axes = plt.subplots(len(METRICS), 1, figsize=(15, 11), layout='constrained')
    labels = [f"{g['placement']} / {g['variant']}" for g in groups]
    for ax, metric in zip(axes, METRICS):
        data = [g['speedup_external'][metric] for g in groups]
        means = [d['mean'] for d in data]
        ax.errorbar(range(len(data)), means,
                    yerr=[[m - d['min'] for m, d in zip(means, data)],
                          [d['max'] - m for m, d in zip(means, data)]],
                    fmt='o', markersize=3, capsize=2)
        ax.axhline(1, color='gray', linewidth=0.8)
        ax.set_ylabel(metric + ('\ncounted / external CPU cost' if metric == 'timed_process_cpu_s'
                                else '\nexternal benefit ratio'))
        ax.set_xlim(-0.5, len(data) - 0.5)
    axes[-1].set_xticks(range(len(labels)), labels, rotation=70, ha='right', fontsize=7)
    fig.suptitle(f"Admission comparison: {'SMOKE' if manifest['smoke'] else 'cold-start fixed work'}; points=paired means, bars=observed min–max (not CI)")
    fig.savefig(analysis_dir / 'comparison.png', dpi=140)
    plt.close(fig)
    return analysis_dir / 'summary.json'


def run_study(options, manifest):
    root = options.output_root
    if root.exists():
        raise ValueError('refusing existing output root: ' + str(root))
    root.mkdir(parents=True)
    binaries = {v: build_provenance(v, options.binary_root / ('upscaledb-' + v),
                                    options.binary_root / ('build-' + v + '.json'))
                for v in options.backends}
    manifest['machine'] = environment(sorted({cpu for case in manifest['placements'] for cpu in case['cpus']}))
    manifest['machine']['discovered_topology'] = manifest['topology']
    mems = next((line.split(':', 1)[1].strip() for line in
                 (read_text('/proc/self/status') or '').splitlines() if line.startswith('Mems_allowed_list:')), None)
    check_memory_selection('bind', [0], parse_cpu_ranges(mems))
    manifest['artifacts'] = {'binaries': binaries, 'native_harness_sha256': digest(HERE / 'native_harness.cc'),
                             'bridge_header_sha256': digest(HERE / 'bridge.h'),
                             'native_profile_patch_sha256': digest(HERE / 'native-lock-timing.patch')}
    manifest['integration_sources'] = capture_sources(root, options.backends)
    manifest['artifacts']['integration_sources'] = manifest['integration_sources']
    manifest['source_files'] = source_files(manifest['artifacts'])
    manifest['frozen_files'] = frozen_files(manifest['artifacts'])
    manifest['study_sources'] = archive_study_sources(root)
    manifest['source_snapshot'] = source_snapshot(root)
    if 'error' in manifest['source_snapshot']:
        raise ValueError(manifest['source_snapshot']['error'])
    manifest['frozen_archive'] = archive_files(root, manifest['source_files'] | manifest['frozen_files'])
    assert_frozen(manifest)
    atomic_new(root / 'manifest.json', manifest)
    signal.signal(signal.SIGINT, request_stop)
    signal.signal(signal.SIGTERM, request_stop)
    baseline = None
    for item in manifest['schedule']:
        command = manifest['commands'][item['file']]
        failure = None
        try:
            assert_frozen(manifest)
        except ValueError as exc:
            failure = str(exc)
        if failure is None:
            result = run_trial(command, options.timeout_seconds,
                               binaries[item['variant']]['sha256'])
        else:
            result = {'command': command, 'returncode': None, 'stdout': '', 'stderr': '',
                      'stdout_base64': '', 'stderr_base64': '', 'result': None,
                      'binary_sha256_before': digest(Path(command[0])),
                      'binary_sha256_after': digest(Path(command[0])), 'success': False,
                      'artifact_error': failure}
        try:
            assert_frozen(manifest)
        except ValueError as exc:
            failure = str(exc)
        result['freeze_error'] = failure
        result['success'] = result['success'] and failure is None
        record = {'schema': 1, 'item': item, 'variant': item['variant'],
                  'admission': item['admission'], 'block': item['block'],
                  'config': manifest['configs'][item['placement']], 'machine': manifest['machine'],
                  'artifacts': manifest['artifacts'], **result}
        atomic_new(root / item['file'], record)
        if not record['success']:
            raise ValueError('failed trial retained: ' + str(root / item['file']))
        try:
            baseline = record_valid(record, item, manifest, baseline)['flags']
        except (ValueError, KeyError, TypeError) as exc:
            raise ValueError('invalid trial retained: ' + str(root / item['file']) + ': ' + str(exc)) from exc
    return analyze_saved(root, root / 'analysis')


def require_counted_admission(binary_root, backends):
    """The integrated harness supports only external admission; never mislabel those results."""
    for backend in backends:
        binary = binary_root / ('upscaledb-' + backend)
        try:
            help_result = subprocess.run([str(binary), '--help'], capture_output=True,
                                         text=True, timeout=10, check=False)
        except (OSError, subprocess.TimeoutExpired) as exc:
            raise ValueError(f'{backend}: historical counted-admission binary unavailable: {exc}') from exc
        if help_result.returncode or '--admission' not in (help_result.stdout + help_result.stderr):
            raise ValueError(f'{backend}: binary lacks historical --admission counted|external ABI; '
                             'current build.py/native_harness.cc supports external admission only. '
                             'Use an independently retained admission-capable historical build.')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary-root', type=Path, default=DEFAULT_BINARY)
    parser.add_argument('--output-root', type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument('--plan-only', action='store_true')
    parser.add_argument('--analyze-only', action='store_true')
    parser.add_argument('--analysis-dir', type=Path, help='fresh directory for read-only raw reanalysis')
    parser.add_argument('--order-seed', type=int, default=20260924)
    parser.add_argument('--timeout-seconds', type=float, default=900)
    parser.add_argument('--smoke', action='store_true', help='explicitly separate small smoke from primary study')
    parser.add_argument('--smoke-repetitions', type=int, default=1)
    parser.add_argument('--smoke-reads', type=int, default=200)
    parser.add_argument('--smoke-inserts', type=int, default=200)
    parser.add_argument('--smoke-preload', type=int, default=100)
    parser.add_argument('--smoke-backends', default='bridge_mutex', help='comma-separated subset (smoke only)')
    args = parser.parse_args(argv)
    args.binary_root = args.binary_root.expanduser().resolve()
    args.output_root = args.output_root.expanduser().resolve()
    if args.plan_only and args.analyze_only or args.analysis_dir and not args.analyze_only:
        parser.error('--plan-only, --analyze-only and --analysis-dir have incompatible roles')
    if args.analyze_only:
        if args.smoke:
            parser.error('smoke classification is read from the frozen manifest')
        print(analyze_saved(args.output_root, (args.analysis_dir or (args.output_root / 'analysis')).expanduser().resolve()))
        return 0
    if not math.isfinite(args.timeout_seconds) or not 0 < args.timeout_seconds < 86400:
        parser.error('timeout must be positive, finite, and under one day')
    if args.smoke:
        args.backends = args.smoke_backends.split(',')
        args.repetitions, args.reads, args.inserts, args.preload = (
            args.smoke_repetitions, args.smoke_reads, args.smoke_inserts, args.smoke_preload)
        if (not 1 <= args.repetitions <= 2 or not 1 <= args.reads <= 2000000 or
                not 1 <= args.inserts <= 2000000 or args.reads + args.inserts > 10000 or
                not 1 <= args.preload <= 10000 or not args.backends or
                len(args.backends) != len(set(args.backends)) or
                any(b not in BACKENDS for b in args.backends)):
            parser.error('smoke requires distinct known backends, 1–2 repetitions, <=10000 total operations and preload')
    else:
        if (args.smoke_repetitions, args.smoke_reads, args.smoke_inserts, args.smoke_preload,
                args.smoke_backends) != (1, 200, 200, 100, 'bridge_mutex'):
            parser.error('small-work overrides require --smoke and a separate output root')
        args.backends = BACKENDS
        args.repetitions, args.reads, args.inserts, args.preload = 5, 400000, 400000, 100000
    require_counted_admission(args.binary_root, args.backends)
    manifest = planned_manifest(args, discover_topology())
    if args.plan_only:
        print(json.dumps(manifest, indent=2, sort_keys=True))
    else:
        print(run_study(args, manifest))
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except TrialStop as exc:
        sys.exit(128 + exc.signum)
    except (ValueError, OSError) as exc:
        sys.exit(str(exc))
