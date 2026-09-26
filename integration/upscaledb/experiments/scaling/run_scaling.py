#!/usr/bin/env python3
"""Plan/run serial, immutable, fresh-process UpScaleDB scaling/NUMA/SMT cases.

--plan-only prints an exact topology-derived case/runner-command manifest without
building or running binaries. A failed/incomplete case is retained, never resumed
or overwritten. The default fast stage is exploratory (three reps, no CI).
"""
import argparse
import json
from pathlib import Path
import subprocess
import sys

from integration.upscaledb._paths import ROOT, RUNNER, REPORTS
from integration.upscaledb.runner.run_trials import atomic_new, digest, discover_topology, read_text

VARIANTS = ('native', 'fc', 'fc_pq', 'uscl', 'cfl_local')
DEFAULT_BINARIES = ROOT / '.worktree' / 'upscaledb-scaling-build'
DEFAULT_OUTPUT = ROOT / '.worktree' / 'upscaledb-scaling-fast'


def select_cpus(topology, count, placement='compact', smt=False, socket=None):
    """Select distinct physical cores, optionally one extra distinct sibling/core.

    Worker ID order is role-balanced across sockets: first W/2 finders, next
    W/2 inserters (or one alternating worker). SMT second half reuses the same
    selected physical cores but never the same logical CPU.
    """
    if count < 1:
        raise ValueError('at least one physical core is required')
    by_socket = {}
    for members in topology['cores'].values():
        first = topology['cpus'][str(members[0])]
        group = (first['socket'], first['node'])
        by_socket.setdefault(group, []).append(sorted(members))
    for groups in by_socket.values():
        groups.sort(key=lambda members: (topology['cpus'][str(members[0])]['core'], members[0]))
    keys = sorted(by_socket)
    if socket is not None:
        keys = [key for key in keys if key[0] == socket]
        if len(keys) != 1:
            raise ValueError(f'socket {socket} must identify exactly one node')
    if not keys:
        raise ValueError('no eligible physical cores')
    if placement == 'balanced':
        if socket is not None or count % len(keys):
            raise ValueError('balanced placement needs an equal core count per socket')
        per = count // len(keys)
        if any(len(by_socket[key]) < per for key in keys):
            raise ValueError('insufficient allowed cores on a socket for balanced placement')
        cores = [by_socket[key][index] for index in range(per) for key in keys]
    elif placement == 'compact':
        cores = [core for key in keys for core in by_socket[key]][:count]
        # Keep the same compact *core set*, but round-robin IDs across sockets
        # once the first socket is full; avoid confounding finder/inserter roles.
        chosen = {key: [core for core in cores if
                  (topology['cpus'][str(core[0])]['socket'],
                   topology['cpus'][str(core[0])]['node']) == key] for key in keys}
        cores = [chosen[key][index] for index in range(count)
                 for key in keys if index < len(chosen[key])]
    else:
        raise ValueError('unknown placement: ' + placement)
    if len(cores) != count:
        raise ValueError(f'insufficient distinct eligible physical cores ({len(cores)} < {count})')
    primary = [core[0] for core in cores]
    if smt:
        if any(len(core) < 2 for core in cores):
            raise ValueError('requested SMT sibling not allowed for each chosen core')
        cpus = primary + [core[1] for core in cores]
    else:
        cpus = primary
    if len(cpus) != len(set(cpus)):
        raise ValueError('logical CPU assigned more than once')
    return cpus


def make_cases(topology):
    """Exact fast-stage matrix: 17 fixed cases and 5 representative duration cases."""
    socket_keys = sorted({(v['socket'], v['node']) for v in topology['cpus'].values()})
    if socket_keys != [(0, 0), (1, 1)]:
        raise ValueError('fast-stage bind0/remote-socket1 labels require socket0/node0 and socket1/node1')
    _, node0 = socket_keys[0]
    second_socket, node1 = socket_keys[1]
    cases = []

    def add(name, phase, count, placement='compact', smt=False, socket=None,
            policy='bind', nodes=None):
        cpus = select_cpus(topology, count, placement, smt, socket)
        memory_nodes = [node0] if nodes is None else list(nodes)
        init_candidates = [cpu for cpu in topology['allowed_cpus']
                           if topology['cpus'][str(cpu)]['node'] == node0]
        if not init_candidates:
            raise ValueError('initialization node lacks an allowed CPU')
        init_cpu = cpus[0] if topology['cpus'][str(cpus[0])]['node'] == node0 else init_candidates[0]
        workers = len(cpus)
        cases.append({'name': name, 'phase': phase, 'mode': phase,
                      'family': 'numa' if name.startswith('fixed-numa') else
                                'smt' if smt else 'physical',
                      'physical_cores': count, 'physical_core_count': count,
                      'workers': workers, 'roles': '1' if workers == 1 else f'{workers // 2}+{workers // 2}',
                      'placement': placement, 'layout': 'packed' if placement == 'compact' else 'split',
                      'smt': smt, 'socket': socket, 'cpus': cpus, 'init_cpu': init_cpu,
                      'init_node': node0, 'memory_policy': policy, 'memory_nodes': memory_nodes,
                      'interpretation': ('one alternating worker' if workers == 1 else
                                         'equal dedicated finder/inserter roles')})

    for count in (1, 2, 4, 8, 16, 32, 64):
        add(f'fixed-physical-compact-w{count}', 'fixed', count)
    for count in (8, 32):
        add(f'fixed-physical-balanced-w{count}', 'fixed', count, 'balanced')
    for count in (4, 16, 32, 64):
        add(f'fixed-smt-compact-c{count}-w{2*count}', 'fixed', count, smt=True)
    add('fixed-smt-balanced-c16-w32', 'fixed', 16, 'balanced', smt=True)
    add('fixed-numa-remote-socket1-w8-bind0', 'fixed', 8, socket=second_socket)
    add('fixed-numa-compact-w8-interleave', 'fixed', 8, nodes=[node0, node1], policy='interleave')
    add('fixed-numa-balanced-w8-interleave', 'fixed', 8, 'balanced', nodes=[node0, node1], policy='interleave')
    add('duration-physical-compact-w8', 'duration', 8)
    add('duration-physical-balanced-w8', 'duration', 8, 'balanced')
    add('duration-smt-compact-c4-w8', 'duration', 4, smt=True)
    add('duration-physical-compact-w64', 'duration', 64)
    add('duration-smt-compact-c64-w128', 'duration', 64, smt=True)
    if len(cases) != 22 or len({case['name'] for case in cases}) != 22:
        raise ValueError('fast-stage case matrix incomplete or repeated')
    return cases


def runner_command(case, args):
    command = [sys.executable, '-m', 'integration.upscaledb.runner.run_trials', '--variants', ','.join(args.variants),
               '--mode', case['phase'], '--roles', case['roles'],
               '--cpus', ','.join(map(str, case['cpus'])),
               '--layout', 'packed' if case['placement'] == 'compact' else 'split',
               '--exclusive-cpus', '--stop-on-failure', '--init-cpu', str(case['init_cpu']),
               '--init-node', str(case['init_node']), '--memory-policy', case['memory_policy'],
               '--memory-nodes', ','.join(map(str, case['memory_nodes'])),
               '--repetitions', str(args.repetitions), '--warmup', str(args.warmup),
               '--reads', str(args.reads), '--inserts', str(args.inserts),
               '--memory-limit-gib', str(args.memory_limit_gib),
               '--seconds', str(args.seconds), '--output-dir', str(args.output_root / case['name'])]
    for variant in args.variants:
        command += ['--binary', f'{variant}={args.binary_root / ("upscaledb-" + variant)}',
                    '--build-manifest', f'{variant}={args.binary_root / ("build-" + variant + ".json")}']
    return command


def artifact_identities(binary_root, variants):
    """Freeze selected executable and authoritative build-manifest bytes."""
    return {variant: {
        'binary_sha256': digest(binary_root / ('upscaledb-' + variant)),
        'build_manifest_sha256': digest(binary_root / ('build-' + variant + '.json'))
    } for variant in variants}


def finished_case(directory, case, command, identities=None):
    """Only a complete, unchanged successful raw case can be skipped on resume."""
    marker = json.loads((directory / 'case-complete.json').read_text())
    if marker['case'] != case or marker['command'] != command:
        raise ValueError('completed case no longer matches current plan: ' + str(directory))
    files = marker['sha256']
    if not files or any(not isinstance(name, str) or '/' in name or name.startswith('.') or
                        (directory / name).is_symlink() or digest(directory / name) != sha
                        for name, sha in files.items()):
        raise ValueError('completed case files changed or missing: ' + str(directory))
    manifest = json.loads((directory / 'manifest.json').read_text())
    if set(manifest['trial_files']) - set(files) or len(manifest['trial_files']) != (
            manifest['config']['repetitions'] * len(manifest['config']['variants'])):
        raise ValueError('completed case raw trial matrix incomplete: ' + str(directory))
    if identities is not None:
        built = manifest.get('artifacts', {}).get('binaries', {})
        if set(built) != set(identities) or any(
                built[variant].get('sha256') != frozen['binary_sha256'] or
                built[variant].get('build_manifest_sha256') != frozen['build_manifest_sha256']
                for variant, frozen in identities.items()):
            raise ValueError('case binary/build provenance differs from frozen study identities')
    for name in manifest['trial_files']:
        rec = json.loads((directory / name).read_text())
        if rec.get('success') is not True or rec.get('command') != manifest['commands'][name]:
            raise ValueError('completed case contains failed/mismatched raw trial: ' + name)


def finalize_case(directory, case, command, identities=None):
    manifest = json.loads((directory / 'manifest.json').read_text())
    files = ['manifest.json', 'integration-sources.tar.gz', 'source-snapshot.tar.gz'] + manifest['trial_files']
    hashes = {name: digest(directory / name) for name in files}
    if any(sha is None for sha in hashes.values()):
        raise ValueError('case artifact disappeared: ' + str(directory))
    atomic_new(directory / 'case-complete.json', {'schema': 1, 'case': case,
               'command': command, 'sha256': hashes})
    finished_case(directory, case, command, identities)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary-root', type=Path, default=DEFAULT_BINARIES)
    parser.add_argument('--output-root', type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument('--variants', default=','.join(VARIANTS), help='comma-separated selected primary variants')
    parser.add_argument('--phases', default='fixed,duration', help='fixed,duration or subset')
    parser.add_argument('--cases', help='comma-separated exact case names (default all in selected phases)')
    parser.add_argument('--repetitions', type=int, default=3)
    parser.add_argument('--warmup', type=float, default=0.5)
    parser.add_argument('--reads', type=int, default=400000)
    parser.add_argument('--inserts', type=int, default=400000)
    parser.add_argument('--seconds', type=float, default=10)
    parser.add_argument('--memory-limit-gib', type=int, default=8,
                        help='explicit per-process virtual-address-space cap, not an RSS limit')
    parser.add_argument('--plan-only', action='store_true')
    parser.add_argument('--resume', action='store_true', help='skip only immutable, fully completed matching cases')
    args = parser.parse_args()
    args.binary_root = args.binary_root.expanduser().resolve()
    args.output_root = args.output_root.expanduser().resolve()
    args.variants = args.variants.split(',')
    phases = args.phases.split(',')
    if not args.variants or len(set(args.variants)) != len(args.variants) or any(
            variant not in VARIANTS for variant in args.variants):
        parser.error('only distinct known fast-stage primary variants may be selected')
    if not phases or len(set(phases)) != len(phases) or any(p not in ('fixed', 'duration') for p in phases):
        parser.error('phases must be a distinct subset of fixed,duration')
    if args.repetitions < 1 or not (0 <= args.warmup <= 60 and 0.01 <= args.seconds <= 600) or not (
            0 < args.reads <= 2000000 and 0 < args.inserts <= 200000000):
        parser.error('workload outside runner/harness limits')
    if not 1 <= args.memory_limit_gib <= 64:
        parser.error('memory-limit-gib must be in [1, 64]')
    try:
        topology = discover_topology()
        cases = [case for case in make_cases(topology) if case['phase'] in phases]
    except (ValueError, OSError) as exc:
        parser.error(str(exc))
    if args.cases:
        selected = args.cases.split(',')
        if len(set(selected)) != len(selected) or any(name not in {case['name'] for case in cases}
                                                      for name in selected):
            parser.error('unknown/repeated case or case not in selected phase')
        cases = [case for case in cases if case['name'] in selected]
    commands = {case['name']: runner_command(case, args) for case in cases}
    identities = artifact_identities(args.binary_root, args.variants)
    plan = {'schema': 1, 'stage': 'exploratory_fast_scaling', 'topology': topology,
            'numa_balancing': read_text('/proc/sys/kernel/numa_balancing'),
            'mems_allowed_list': next((line.partition(':')[2].strip() for line in
                (read_text('/proc/self/status') or '').splitlines() if line.startswith('Mems_allowed_list:')), None),
            'binary_root': str(args.binary_root), 'output_root': str(args.output_root),
            'config': {'variants': args.variants, 'phases': phases, 'repetitions': args.repetitions,
                       'warmup': args.warmup, 'reads': args.reads, 'inserts': args.inserts,
                       'memory_limit_gib': args.memory_limit_gib,
                       'duration_seconds': args.seconds, 'timing_policy':
                       f'fixed_{args.reads}_reads_{args.inserts}_inserts_or_duration_'
                       f'{args.seconds}_seconds_before_deadline'},
            'cases': cases, 'runner_commands': commands, 'binary_artifacts': identities,
            'runner_sha256': digest(RUNNER / 'run_trials.py'), 'analyzer_sha256': digest(REPORTS / 'analyze_trials.py'),
            'scaling_sha256': digest(Path(__file__))}
    if args.plan_only:
        print(json.dumps(plan, indent=2, sort_keys=True))
        return 0
    if any(not item['binary_sha256'] or not item['build_manifest_sha256']
           for item in identities.values()):
        parser.error('all selected executable/build-manifest files must exist for execution')
    if args.resume:
        if not (args.output_root / 'study.json').is_file():
            parser.error('resume requires an existing study manifest')
        saved = json.loads((args.output_root / 'study.json').read_text())
        if saved != plan:
            parser.error('existing immutable study manifest differs from requested plan/topology/source')
    else:
        if args.output_root.exists() and any(args.output_root.iterdir()):
            parser.error('refusing nonempty output root (use --resume only for completed matching cases)')
        args.output_root.mkdir(parents=True, exist_ok=True)
        atomic_new(args.output_root / 'study.json', plan)
    for case in cases:
        if artifact_identities(args.binary_root, args.variants) != identities:
            raise ValueError('binary/build-manifest bytes changed since study manifest; refusing mixed cases')
        name = case['name']
        directory = args.output_root / name
        command = commands[name]
        if directory.exists():
            if not args.resume:
                raise ValueError('refusing existing case directory: ' + str(directory))
            finished_case(directory, case, command, identities)
            print('Retaining completed immutable case:', name, flush=True)
            continue
        print('Starting serial case:', name, flush=True)
        result = subprocess.run(command, cwd=ROOT, check=False)
        if result.returncode:
            raise RuntimeError(f'case {name} failed ({result.returncode}); retained: {directory}')
        if artifact_identities(args.binary_root, args.variants) != identities:
            raise ValueError('binary/build-manifest bytes changed during case; retaining raw data')
        finalize_case(directory, case, command, identities)
    # Analysis is deliberately deferred until ALL timed cases have finished;
    # plotting is left to the cross-case report, outside the measurement stage.
    for case in cases:
        directory = args.output_root / case['name']
        summary_path = directory / 'analysis' / 'summary.json'
        if not summary_path.is_file():
            analyzed = subprocess.run([sys.executable, '-m', 'integration.upscaledb.reports.analyze_trials', '--no-plots',
                                       '--input-dir', str(directory)], cwd=ROOT, check=False)
            if analyzed.returncode:
                raise RuntimeError(f'analysis for {case["name"]} failed ({analyzed.returncode}); retained: {directory}')
        summary = json.loads(summary_path.read_text())
        if summary['failure_count']:
            raise ValueError(f'analysis retained invalid raw trials in {directory}')
    print('Study manifest:', args.output_root / 'study.json')
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (ValueError, RuntimeError, OSError) as exc:
        print(str(exc), file=sys.stderr)
        sys.exit(1)
