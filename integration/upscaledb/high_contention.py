#!/usr/bin/env python3
"""Prepare, measure and analyze the approved high-contention UpScaleDB cohort.

Run under the shared measurement lock for preparation/analysis, exclusive for
both timed modes, and under the other-tables lock and taskset -c32-63. Never
reuse an output root: failed invocations and all previous cohorts stay intact.
"""
import argparse
import itertools
import math
from pathlib import Path
import random
import signal
import sys

import boundary_tables as base

SCHEMA = 'upscaledb-high-contention-v1'
WORKERS = (8, 16, 32)
ROUTING = ('uniform', 'hot90', 'hot100')
DIAGNOSTIC_BACKENDS = ('fc', 'fc_pq', 'mcs')
CPUS = list(range(32, 64))
REPETITIONS = 5
DIAGNOSTIC_REPETITIONS = 3
HYPOTHESES = [
    'Greater per-gate contention may improve combining amortization, but is not guaranteed.',
    'Changing hot-table routing also changes database key and cache locality; it is not a pure lock-cost intervention.',
]
CAVEATS = [
    'Fixed 32 separate in-memory environments, 32768 total initial records and unique alternating find/insert streams.',
    'Routing targets are seed-dependent probabilities; report observed per-table counts, not an exact 90% outcome.',
    'Eight/16/32 workers are pinned on distinct physical node1 cores, without SMT siblings or oversubscription.',
    '32GiB virtual address-space cap; four million unique inserts per worker; hitting cap fails the measurement.',
    'Live-worker eight-sweep find-only registration touches all 32 handles before release, including in hot100.',
    'Window throughput uses on-time completions; latency, CPU, and count/oracle also include the drained request.',
    'API overlap diagnostics use separate binaries with atomic per-table counters; not lock wait depth or primary timing.',
    'Five paired repeats yield descriptive all-ratios >1.05 or <0.95 screens, not statistical significance.',
    'No heterogeneous service-fairness or FC-PQ win claim follows from this alternating equal-cost workload.',
    'CFL-local is a local proxy with global accounting across handles, not a verified paper implementation.',
    'CLH retains queue nodes without Drop; Shfl remains excluded for documented source-level safety failures.',
]


def validate(result, item, seconds=2, errors=False):
    base.validate(result, item, seconds, errors)
    base.require(item['workers'] in WORKERS and item['routing'] in ROUTING and
                 item['layout'] == 'split' and item['tables'] == 32, 'incorrect contention shape')
    base.require(result['workers_count'] == item['workers'] and result['routing'] == item['routing'] and
                 result['api_overlap_diagnostic'] is item['diagnostic'], 'mode or routing mismatch')
    if errors:
        return
    base.require(result['max_inserts_total'] // result['workers_count'] == 4000000,
                 'validated insert limit mismatch')
    for worker in result['workers']:
        counts = worker.get('api_overlap_counts')
        if item['diagnostic']:
            base.require(isinstance(counts, list) and len(counts) == 32, 'missing overlap tables')
            for table, histogram in zip(worker['tables'], counts):
                base.require(len(histogram) == item['workers'] and
                             all(isinstance(n, int) and n >= 0 for n in histogram),
                             'invalid overlap histogram')
                base.require(sum(histogram) == table['finds'] + table['inserts'],
                             'overlap submissions/completions disagree')
        else:
            base.require(counts is None, 'shared diagnostic counters leaked into primary build')


def validate_manifest(manifest):
    base.require(manifest['schema'] == SCHEMA and manifest['experiment'] == 'upscaledb-high-contention',
                 'unexpected experiment identity')
    backends = tuple(manifest['backends'])
    base.require(backends == base.BACKENDS and manifest['excluded_backends'] == base.SAFETY_EXCLUSIONS,
                 'matrix must retain all ten safe backends and predeclared Shfl exclusion')
    base.require(manifest['repetitions'] == REPETITIONS and manifest['seconds'] == 2 and
                 manifest['expected_trials'] == REPETITIONS * len(WORKERS) * len(ROUTING) * len(backends),
                 'primary matrix size mismatch')
    base.require(manifest['expected_diagnostics'] == DIAGNOSTIC_REPETITIONS * len(ROUTING) *
                 len(DIAGNOSTIC_BACKENDS), 'diagnostic matrix size mismatch')
    base.require(manifest['smoke_invocations'] == len(backends) * len(WORKERS) * (len(ROUTING) + 1) +
                 len(DIAGNOSTIC_BACKENDS) * (len(ROUTING) + 1), 'gate count mismatch')
    base.require(set(manifest['builds']) == set(backends) | {b + '-diagnostic' for b in DIAGNOSTIC_BACKENDS},
                 'unexpected binaries')
    base.require(manifest['configuration']['cpus'] == CPUS and
                 manifest['configuration']['max_inserts_total_by_workers'] ==
                 {str(n): n * 4000000 for n in WORKERS}, 'placement/insert bounds mismatch')
    rng = random.Random(manifest['shuffle_seed'])
    seeds = [rng.randrange(1, 2**63) for _ in range(REPETITIONS)]
    base.require(manifest['workload_seeds'] == seeds, 'paired seeds changed')
    expected = []
    for rep, seed in enumerate(seeds, 1):
        cells = list(itertools.product(WORKERS, ROUTING, backends))
        rng.shuffle(cells)
        for workers, routing, backend in cells:
            expected.append((rep, seed, workers, routing, backend, False))
    diagnostic_rng = random.Random(manifest['shuffle_seed'] ^ 0x5216fb81)
    diagnostics = []
    for rep, seed in enumerate(seeds[:DIAGNOSTIC_REPETITIONS], 1):
        cells = list(itertools.product(ROUTING, DIAGNOSTIC_BACKENDS))
        diagnostic_rng.shuffle(cells)
        diagnostics.extend((rep, seed, 32, routing, backend, True) for routing, backend in cells)
    for prefix, actual, expected_rows in (('trial', manifest['schedule'], expected),
                                          ('diagnostic', manifest['diagnostic_schedule'], diagnostics)):
        base.require(len(actual) == len(expected_rows), 'schedule size mismatch')
        for index, (item, (rep, seed, workers, routing, backend, diagnostic)) in enumerate(zip(actual, expected_rows), 1):
            binary = manifest['builds'][backend + ('-diagnostic' if diagnostic else '')]['binary']
            base.require(item == {'id': f'{prefix}-{index:03d}', 'rep': rep, 'seed': seed,
                                  'layout': 'split', 'tables': 32, 'workers': workers,
                                  'routing': routing, 'backend': backend, 'diagnostic': diagnostic,
                                  'command': base.command(binary, 'split', 32, seed,
                                                          workers=workers, routing=routing)},
                         'predeclared schedule, order, or command changed')
    for backend in DIAGNOSTIC_BACKENDS:
        primary = manifest['builds'][backend]
        instrumented = manifest['builds'][backend + '-diagnostic']
        expected_command = list(primary['command'])
        expected_command[expected_command.index('-o') + 1] = instrumented['binary']
        expected_command.insert(1, '-DUPS_API_OVERLAP_DIAGNOSTIC=1')
        base.require(instrumented['command'] == expected_command and
                     '-DUPS_API_OVERLAP_DIAGNOSTIC=1' not in primary['command'],
                     'diagnostic compiler instrumentation leaked into primary')
    return backends


EXPERIMENT = {'schema': SCHEMA, 'name': 'upscaledb-high-contention', 'workers': WORKERS,
              'routing': ROUTING, 'cpus': CPUS, 'repetitions': REPETITIONS,
              'diagnostic_backends': DIAGNOSTIC_BACKENDS, 'hypotheses': HYPOTHESES,
              'caveats': CAVEATS, 'validator': validate, 'matrix_validator': validate_manifest}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-root', type=Path, required=True)
    parser.add_argument('--binary-root', type=Path, default=base.ROOT / '.worktree/upscaledb-high-contention-build')
    parser.add_argument('--frozen-root', type=Path, default=base.FROZEN_ROOT,
                        help='verified native/FC baseline build root')
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--prepare-only', action='store_true')
    mode.add_argument('--run', action='store_true', help='primary 450 only')
    mode.add_argument('--run-diagnostics', action='store_true', help='separate 27 instrumented processes only')
    mode.add_argument('--analyze-only', action='store_true', help='saved results, no experiment subprocesses')
    parser.add_argument('--shuffle-seed', type=int, default=2026092501)
    parser.add_argument('--timeout-seconds', type=float, default=600)
    args = parser.parse_args()
    args.output_root = args.output_root.expanduser().resolve()
    args.binary_root = args.binary_root.expanduser().resolve()
    args.frozen_root = args.frozen_root.expanduser().resolve()
    args.backends = base.BACKENDS
    args.exclusion_reason = None
    base.require(1 <= args.shuffle_seed < 2**63, 'shuffle seed out of range')
    base.require(math.isfinite(args.timeout_seconds) and 1 <= args.timeout_seconds <= 3600,
                 'watchdog must be in [1,3600] seconds')
    signal.signal(signal.SIGINT, base.interrupt)
    signal.signal(signal.SIGTERM, base.interrupt)
    if args.prepare_only:
        base.prepare(args, EXPERIMENT)
        print(args.output_root / 'prepared.json')
    elif args.run:
        base.run(args.output_root, EXPERIMENT)
        print(args.output_root / 'run/status.json')
    elif args.run_diagnostics:
        base.run(args.output_root, EXPERIMENT, diagnostic=True)
        print(args.output_root / 'diagnostics/status.json')
    else:
        import high_contention_report
        print(high_contention_report.analyze(args.output_root))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, InterruptedError, KeyboardInterrupt) as exc:
        sys.exit(type(exc).__name__ + ': ' + str(exc))
