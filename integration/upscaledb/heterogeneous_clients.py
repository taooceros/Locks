#!/usr/bin/env python3
"""Run fresh-process heterogeneous-client trials without building or hiding failures.

External coordinator owns measurement/slot locks and taskset; do not nest them.
Build separately with build.py --variants hetero_native,... --output-root BUILD.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import subprocess
import sys
import time
from build import rust_sources_digest

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
BACKENDS = ('native', 'bridge_mutex', 'fc', 'fc_pq', 'uscl', 'cfl_local', 'mcs')
MIXES = ('reads', 'single', 'batch')
PLACEMENTS = ('shared', 'split')
SEEDS = (101, 202, 303)
CPUS = '0,1,2,3,4,5,6,7'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def preflight(build_root, cpu_list=CPUS, memory_node=0):
    build_root = build_root.resolve(strict=True)
    try:
        cpus = [int(c) for c in cpu_list.split(',')]
    except ValueError as exc:
        raise ValueError('worker CPUs must be eight comma-separated integers') from exc
    if len(cpus) != 8 or len(set(cpus)) != 8:
        raise ValueError('worker CPUs must be eight distinct physical CPU IDs')
    if not set(cpus).issubset(os.sched_getaffinity(0)):
        raise ValueError('worker CPUs unavailable in parent affinity')
    node = Path(f'/sys/devices/system/node/node{memory_node}/cpulist').read_text().strip()
    spans = [tuple(map(int, part.split('-'))) for part in node.split(',')]
    node_cpus = {n for a, *last in spans for n in range(a, (last or [a])[0] + 1)}
    siblings = []
    for cpu in cpus:
        if cpu not in node_cpus:
            raise ValueError(f'CPU{cpu} is not on memory node {memory_node}')
        siblings.append(Path(f'/sys/devices/system/cpu/cpu{cpu}/topology/thread_siblings_list').read_text().strip())
    if len(set(siblings)) != 8:
        raise ValueError('worker CPU list includes SMT siblings')
    variants = {}
    rust_source_hash = rust_sources_digest()
    source_inputs = {
        'heterogeneous_clients.py': HERE / 'heterogeneous_clients.py',
        'heterogeneous_ops.patch': HERE / 'heterogeneous-ops.patch',
        'bridge_ops.patch': HERE / 'bridge-ops.patch',
        'heterogeneous_clients.cc': HERE / 'heterogeneous_clients.cc',
        'build.py': HERE / 'build.py', 'bridge.h': HERE / 'bridge.h',
        'heterogeneous_clients_analysis.py': HERE / 'heterogeneous_clients_analysis.py',
        'private_ops.h': HERE / 'private_ops.h',
        'adapter.rs': ROOT / 'crates/upscaledb-bridge/src/lib.rs',
        'mcs.rs': ROOT / 'crates/libdlock/src/dlock2/mcs.rs',
        'fc.rs': ROOT / 'crates/libdlock/src/dlock2/fc/lock.rs',
        'fc_pq.rs': ROOT / 'crates/libdlock/src/dlock2/fc_pq/lock.rs',
    }
    for name in (*BACKENDS, *(f'{b}_profile' if b != 'native' else 'profile' for b in BACKENDS)):
        variant = 'hetero_' + name
        binary = build_root / ('upscaledb-' + variant)
        manifest_path = build_root / ('build-' + variant + '.json')
        if not binary.is_file() or not manifest_path.is_file():
            raise ValueError(f'missing binary/manifest for {name}')
        manifest = json.loads(manifest_path.read_text())
        hashes = manifest.get('hashes', {})
        if (manifest.get('variant') != variant or manifest.get('source_kind') != 'heterogeneous' or
            manifest.get('binary') != str(binary) or hashes.get('binary_sha256') != sha(binary) or
            hashes.get('build_py_sha256') != sha(HERE / 'build.py') or
            hashes.get('harness_sha256') != sha(HERE / 'heterogeneous_clients.cc') or
            hashes.get('private_ops_sha256') != sha(HERE / 'private_ops.h') or
            hashes.get('bridge_h_sha256') != sha(HERE / 'bridge.h') or
            hashes.get('heterogeneous_patch_sha256') != sha(HERE / 'heterogeneous-ops.patch') or
            manifest.get('source_verification', {}).get('bridge_patch_sha256') !=
                sha(HERE / 'bridge-ops.patch') or
            manifest.get('source_verification', {}).get('heterogeneous_patch_sha256') !=
                sha(HERE / 'heterogeneous-ops.patch')):
            raise ValueError(f'build identity mismatch for {name}')
        source = Path(manifest['source'])
        library = manifest['library_verification']
        if (not source.is_dir() or
            sha(source / 'src/5upscaledb/upscaledb.cc') !=
                manifest['source_verification']['implementation_sha256'] or
            sha(Path(library['shared_object'])) != library['shared_object_sha256']):
            raise ValueError(f'patched native DB source/library changed for {name}')
        archive = manifest.get('rust_staticlib')
        if archive and (sha(Path(archive['archive'])) != archive['archive_sha256'] or
                        archive.get('rust_sources_sha256') != rust_source_hash):
            raise ValueError(f'Rust staticlib/source changed for {name}')
        variants[variant] = {'binary': str(binary), 'binary_sha256': sha(binary),
                             'manifest': str(manifest_path), 'manifest_sha256': sha(manifest_path),
                             'build_manifest': manifest}
    return {'build_root': str(build_root), 'variants': variants,
            'sources_sha256': {name: sha(path) for name, path in source_inputs.items()},
            'sources_paths': {name: str(path) for name, path in source_inputs.items()},
            'topology': {'memory_node_cpulist': node, 'thread_siblings': siblings,
                         'affinity': sorted(os.sched_getaffinity(0))}}


def run(args):
    if args.seconds not in (1, 2) or (not args.smoke and args.seconds != 2):
        raise ValueError('primary duration is exactly 2 seconds; 1 second requires --smoke')
    if args.preload < 2 or args.preload > 1000000 or args.preload % 2:
        raise ValueError('preload must be even and in [2, 1000000]')
    if args.max_records < 32 or args.max_records > 20000000 or args.max_records % 32:
        raise ValueError('max-records must be divisible by32 and in [32, 20000000]')
    if args.timeout < 5 or args.timeout > 600:
        raise ValueError('timeout must be in [5, 600] seconds')
    identity = preflight(args.build_root, args.cpus, args.memory_node)
    root = args.output_root.resolve()
    root.mkdir(parents=True, exist_ok=False)
    cases = [(mix, place, rep, seed, backend, profile)
             for rep, seed in enumerate(SEEDS if not args.smoke else SEEDS[:1], start=1)
             for mix in MIXES for place in PLACEMENTS
             for profile in (False, True)
             for backend in BACKENDS]
    shuffle_seed = 2026092502
    random.Random(shuffle_seed).shuffle(cases)
    config = {'schema': 1, 'purpose': 'UpScaleDB synchronous heterogeneous clients',
              'profiles': 'separate identical workloads with timing instrumentation',
              'mixes': MIXES, 'placements': PLACEMENTS,
              'backends': BACKENDS, 'seeds': SEEDS if not args.smoke else SEEDS[:1],
              'shuffle_seed': shuffle_seed, 'schedule': cases,
              'repetitions': 1 if args.smoke else 3,
              'worker_cpus': args.cpus, 'memory_node': args.memory_node,
              'memory_limit_gib': 8,
              'preload_total': args.preload, 'seconds': args.seconds,
              'max_records': args.max_records, 'smoke': args.smoke,
              'batch_records': 8, 'windows': 8,
              'batch_semantics': 'one nontransactional scheduling unit, eight real inserts',
              'comparison_caveat': 'submission/return gaps captured; no proof of continuously selectable backlog or starvation bound',
              'build': identity, 'host': platform.uname()._asdict(),
              'controller_argv': sys.argv, 'created_epoch_ns': time.time_ns(),
              'trials': len(cases)}
    (root / 'manifest.json').write_text(json.dumps(config, indent=2, sort_keys=True) + '\n')
    failures = 0
    for mix, place, rep, seed, backend, profile in cases:
        variant = 'profile' if backend == 'native' and profile else (
            backend + '_profile' if profile else backend)
        build_variant = 'hetero_' + variant
        label = f'{mix}-{place}-r{rep}-{variant}'
        trial = root / label
        trial.mkdir(exist_ok=False)
        command = ['numactl', f'--membind={args.memory_node}', identity['variants'][build_variant]['binary'],
                   '--mix', mix, '--placement', place, '--cpus', args.cpus,
                   '--seed', str(seed), '--preload', str(args.preload),
                   '--seconds', str(args.seconds), '--max-records', str(args.max_records)]
        (trial / 'command.json').write_text(json.dumps(command, indent=2) + '\n')
        started = time.time_ns()
        stdout, stderr, code, timed_out = '', '', None, False
        try:
            result = subprocess.run(command, cwd=ROOT, capture_output=True,
                                    text=True, errors='replace', timeout=args.timeout)
            stdout, stderr, code = result.stdout, result.stderr, result.returncode
        except subprocess.TimeoutExpired as exc:
            timed_out = True
            stdout = exc.stdout.decode(errors='replace') if isinstance(exc.stdout, bytes) else (exc.stdout or '')
            stderr = exc.stderr.decode(errors='replace') if isinstance(exc.stderr, bytes) else (exc.stderr or '')
        except OSError as exc:
            stderr = str(exc)
        (trial / 'stdout.txt').write_text(stdout)
        (trial / 'stderr.txt').write_text(stderr)
        error = None
        try:
            parsed = json.loads(stdout)
            if (parsed.get('schema') != 1 or parsed.get('variant') != variant or
                parsed.get('mix') != mix or parsed.get('placement') != place or
                parsed.get('seed') != seed or parsed.get('preload_total') != args.preload or
                parsed.get('duration_ns') != args.seconds * 1000000000 or
                parsed.get('max_inserted_records') != args.max_records or
                parsed.get('memory_limit_gib') != 8 or
                parsed.get('profile_enabled') is not profile or len(parsed['workers']) != 8):
                raise ValueError('result does not match requested trial identity')
            (trial / 'result.json').write_text(json.dumps(parsed, indent=2, sort_keys=True) + '\n')
        except (ValueError, KeyError, TypeError, AttributeError) as exc:
            parsed = None
            error = str(exc)
        passed = code == 0 and not timed_out and parsed is not None and parsed['status'] == 'ok'
        (trial / 'status.json').write_text(json.dumps({
            'ok': passed, 'exit_code': code, 'timeout': timed_out, 'parse_error': error,
            'start_epoch_ns': started, 'end_epoch_ns': time.time_ns(),
            'variant': variant, 'mix': mix, 'placement': place, 'rep': rep, 'seed': seed,
            'profile': profile}, indent=2, sort_keys=True) + '\n')
        failures += not passed
        print(label, 'ok' if passed else 'FAILED', flush=True)
    print(f'{len(cases)} trials; {failures} failures; raw root {root}', flush=True)
    return int(failures != 0)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--run', action='store_true')
    mode.add_argument('--analyze-only', action='store_true')
    parser.add_argument('--build-root', type=Path)
    parser.add_argument('--output-root', type=Path, required=True)
    parser.add_argument('--seconds', type=int, default=2)
    parser.add_argument('--preload', type=int, default=40000)
    parser.add_argument('--max-records', type=int, default=5000000)
    parser.add_argument('--timeout', type=int, default=45)
    parser.add_argument('--smoke', action='store_true')
    parser.add_argument('--cpus', default=CPUS,
                        help='eight comma-separated physical CPU IDs on the chosen memory node')
    parser.add_argument('--memory-node', type=int, default=0,
                        help='NUMA memory node containing those CPUs')
    args = parser.parse_args()
    if args.run:
        if args.build_root is None:
            parser.error('--run requires --build-root')
        return run(args)
    from heterogeneous_clients_analysis import analyze
    return analyze(args.output_root)


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError) as exc:
        print('heterogeneous controller:', exc, file=sys.stderr)
        sys.exit(2)
