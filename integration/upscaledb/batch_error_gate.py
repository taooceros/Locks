#!/usr/bin/env python3
"""Compile/run one partial-commit batch gate per built UpScaleDB variant.

Consumes each verified build manifest's exact harness link command; never
rebuilds native DB/Rust archives or overwrites production binaries.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time
from build import HETERO_VARIANTS

HERE = Path(__file__).resolve().parent
SOURCE = HERE / 'batch_error_gate.cc'
HARNESS = HERE / 'heterogeneous_clients.cc'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def call(command, cwd, timeout):
    try:
        result = subprocess.run(command, cwd=cwd, capture_output=True, text=True,
                                errors='replace', timeout=timeout)
        return result.stdout, result.stderr, result.returncode, False
    except subprocess.TimeoutExpired as exc:
        stdout = exc.stdout.decode(errors='replace') if isinstance(exc.stdout, bytes) else (exc.stdout or '')
        stderr = exc.stderr.decode(errors='replace') if isinstance(exc.stderr, bytes) else (exc.stderr or '')
        return stdout, stderr, None, True
    except OSError as exc:
        return '', str(exc), None, False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--build-root', type=Path, required=True)
    parser.add_argument('--output-root', type=Path, required=True)
    parser.add_argument('--variants', nargs='+', choices=HETERO_VARIANTS,
                        default=HETERO_VARIANTS, help='batch-patched variants to gate')
    parser.add_argument('--memory-node', type=int, default=0)
    parser.add_argument('--cpu', type=int, default=0, help='physical setup/requester CPU on --memory-node')
    args = parser.parse_args()
    build = args.build_root.resolve(strict=True)
    root = args.output_root.resolve()
    root.mkdir(parents=True, exist_ok=False)
    metadata = {'schema': 1, 'purpose': 'partial-success and duplicate-stop eight-insert gate',
                'source': str(SOURCE), 'source_sha256': sha(SOURCE),
                'production_harness_sha256': sha(HARNESS),
                'build_root': str(build), 'variants': list(args.variants),
                'argv': sys.argv, 'created_epoch_ns': time.time_ns()}
    (root / 'manifest.json').write_text(json.dumps(metadata, indent=2, sort_keys=True) + '\n')
    errors = 0
    for variant in args.variants:
        directory = root / variant
        directory.mkdir(exist_ok=False)
        manifest_path = build / ('build-' + variant + '.json')
        executable = build / ('upscaledb-' + variant)
        status = {'variant': variant, 'ok': False, 'build_manifest_path': str(manifest_path)}
        try:
            manifest = json.loads(manifest_path.read_text())
            hashes = manifest['hashes']
            if (manifest['schema'] != 2 or manifest['variant'] != variant or
                manifest['source_kind'] != 'heterogeneous' or
                Path(manifest['binary']) != executable or
                hashes['binary_sha256'] != sha(executable) or
                hashes['harness_sha256'] != sha(HARNESS) or
                hashes['build_py_sha256'] != sha(HERE / 'build.py') or
                hashes['bridge_h_sha256'] != sha(HERE / 'bridge.h') or
                hashes['private_ops_sha256'] != sha(HERE / 'private_ops.h') or
                hashes['heterogeneous_patch_sha256'] != sha(HERE / 'heterogeneous-ops.patch') or
                manifest['source_verification']['implementation_sha256'] !=
                    sha(Path(manifest['source']) / 'src/5upscaledb/upscaledb.cc') or
                manifest['library_verification']['shared_object_sha256'] !=
                    sha(Path(manifest['library_verification']['shared_object']))):
                raise ValueError('production build provenance mismatch')
            archive = manifest.get('rust_staticlib')
            if archive and sha(Path(archive['archive'])) != archive['archive_sha256']:
                raise ValueError('Rust bridge archive changed')
            original = manifest['build']['harness_command']
            if original.count(str(HARNESS)) != 1 or original.count('-o') != 1:
                raise ValueError('build manifest has unexpected harness command')
            command = list(original)
            command[command.index(str(HARNESS))] = str(SOURCE)
            smoke_binary = directory / 'batch_error_gate'
            command[command.index('-o') + 1] = str(smoke_binary)
            status['build_manifest_sha256'] = sha(manifest_path)
            status['compile_command'] = command
            stdout, stderr, code, timeout = call(command, HERE, 240)
            (directory / 'compile-stdout.txt').write_text(stdout)
            (directory / 'compile-stderr.txt').write_text(stderr)
            status['compile_exit'] = code
            status['compile_timeout'] = timeout
            if code == 0 and not timeout:
                status['binary_sha256'] = sha(smoke_binary)
                command = ['numactl', f'--membind={args.memory_node}', str(smoke_binary),
                           '--cpu', str(args.cpu)]
                status['run_command'] = command
                stdout, stderr, code, timeout = call(command, HERE, 30)
                (directory / 'stdout.txt').write_text(stdout)
                (directory / 'stderr.txt').write_text(stderr)
                status['run_exit'] = code
                status['run_timeout'] = timeout
                try:
                    result = json.loads(stdout)
                    if (result['status'] != 'ok' or result['variant'] != variant.removeprefix('hetero_') or
                        result['successful_inserts'] != 3 or result['duplicate_index'] != 3 or
                        result['unexecuted_suffix'] != 4 or result['db_count'] != 67 or
                        result['oracle_bad'] != 0):
                        raise ValueError('batch gate result differs from expected partial commit')
                    (directory / 'result.json').write_text(json.dumps(result, indent=2, sort_keys=True) + '\n')
                    status['ok'] = code == 0 and not timeout
                except (ValueError, KeyError, TypeError) as exc:
                    status['parse_error'] = str(exc)
        except (OSError, ValueError, KeyError, TypeError) as exc:
            status['error'] = str(exc)
        (directory / 'status.json').write_text(json.dumps(status, indent=2, sort_keys=True) + '\n')
        errors += not status['ok']
        print(variant, 'ok' if status['ok'] else 'FAILED', flush=True)
    print(f'{len(args.variants) - errors}/{len(args.variants)} batch gates passed; {root}', flush=True)
    return int(bool(errors))


if __name__ == '__main__':
    sys.exit(main())
