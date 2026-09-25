#!/usr/bin/env python3
"""Run real Store/bridge self-tests from independently built variant binaries."""
import argparse
import json
from pathlib import Path
import signal
import subprocess

VARIANTS = ('native', 'refactored', 'bridge_mutex', 'fc', 'fc_pq',
            'uscl', 'cfl_local', 'spinlock', 'mcs', 'ticket', 'clh')
OPTIONAL_VARIANTS = ('bridge_mutex_profile', 'fc_profile', 'fc_pq_profile',
                     'uscl_profile', 'cfl_local_profile', 'spinlock_profile',
                     'mcs_profile', 'ticket_profile', 'clh_profile')


def run(binary, case, failstop=False):
    command = [str(binary), '--self-test', case]
    result = subprocess.run(command, capture_output=True, text=True, timeout=120)
    if failstop:
        if result.returncode != -signal.SIGABRT:
            raise RuntimeError(f'{command}: expected SIGABRT, got {result.returncode}: '
                               f'{result.stdout} {result.stderr}')
        return
    if result.returncode:
        raise RuntimeError(f'{command}: {result.returncode}: {result.stdout} {result.stderr}')
    payload = json.loads(result.stdout)
    if payload != {'self_test': case, 'status': 'ok'}:
        raise RuntimeError(f'{command}: incorrect result: {payload}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bin-dir', type=Path, default=Path(__file__).resolve().parents[2] /
                        '.worktree/upscaledb')
    parser.add_argument('--variants', nargs='+', choices=(*VARIANTS, *OPTIONAL_VARIANTS),
                        help='select independently built variants (default: all primary variants)')
    args = parser.parse_args()
    variants = args.variants if args.variants is not None else VARIANTS
    for variant in variants:
        binary = args.bin_dir / ('upscaledb-' + variant)
        run(binary, 'memory-policy')
        run(binary, 'errors')
        run(binary, 'stress')
    test_binary = args.bin_dir / 'upscaledb-test_hooks'
    for case in ('errors', 'stress', 'expected-exception', 'external-lifecycle'):
        run(test_binary, case)
    for case in ('cpp-throw', 'rust-panic'):
        run(test_binary, case, failstop=True)
    print(json.dumps({'status': 'ok', 'variants': variants,
                      'test_hooks': ('errors', 'stress', 'expected-exception',
                                     'external-lifecycle', 'cpp-throw', 'rust-panic')}))


if __name__ == '__main__':
    main()
