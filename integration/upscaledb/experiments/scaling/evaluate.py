#!/usr/bin/env python3
"""Run the approved matrix serially, preserving every raw trial and failure.

Run only after the integration correctness gate. Existing case directories are
refused, not silently resumed. --phase selects explicit independent stages;
all stages use the same primary defaults. Profile duration runs are confined to
the packed layout and are not primary performance estimates.
"""
import argparse
import json
from pathlib import Path
import subprocess
import sys

from integration.upscaledb._paths import ROOT

PRIMARY = 'native,refactored,bridge_mutex,fc,fc_pq'
PROFILE = 'profile,bridge_mutex_profile,fc_profile,fc_pq_profile'
LAYOUTS = {'packed': '0,1,2,3', 'split': '0,1,32,33'}


def cases(phase):
    if phase in ('all', 'fixed'):
        # Start with the reference 4+4 oversubscribed arrangement, then scaling.
        for layout, cpus in LAYOUTS.items():
            for workers in (8, 1, 2, 4, 16):
                roles = '1' if workers == 1 else f'{workers // 2}+{workers // 2}'
                variants = PRIMARY
                if layout == 'packed' and workers == 8:
                    variants += ',' + PROFILE
                yield f'fixed-{layout}-{workers}w', 'fixed', layout, cpus, roles, variants
    if phase in ('all', 'duration'):
        for layout, cpus in LAYOUTS.items():
            yield f'duration-{layout}-8w', 'duration', layout, cpus, '4+4', PRIMARY
    if phase in ('all', 'profile'):
        yield 'profile-duration-packed-8w', 'duration', 'packed', LAYOUTS['packed'], '4+4', PROFILE


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-root', required=True, type=Path)
    parser.add_argument('--phase', choices=('all', 'fixed', 'duration', 'profile'), default='all')
    args = parser.parse_args()
    out = args.output_root.resolve()
    matrix = list(cases(args.phase))
    for name, *_ in matrix:
        if (out / name).exists():
            parser.error(f'refusing existing case directory: {out / name}')
    out.mkdir(parents=True, exist_ok=True)
    schedule = []
    for name, mode, layout, cpus, roles, variants in matrix:
        command = [sys.executable, '-m', 'integration.upscaledb.runner.run_trials', '--variants', variants,
                   '--mode', mode, '--roles', roles, '--cpus', cpus, '--layout', layout,
                   '--repetitions', '10', '--seconds', '120', '--warmup', '5',
                   '--seed', '1', '--order-seed', '20260923', '--reads', '400000',
                   '--inserts', '400000', '--preload', '100000', '--max-inserts', '200000000',
                   '--memory-limit-gib', '8', '--output-dir', str(out / name)]
        schedule.append({'name': name, 'command': command})
    schedule_path = out / f'schedule-{args.phase}.json'
    with schedule_path.open('x') as stream:
        json.dump({'phase': args.phase, 'cases': schedule,
                   'execution': 'serial fresh processes; stop after a failed case; never overwrite',
                   'profile_duration_scope': 'packed layout only; instrumented, not primary'},
                  stream, indent=2)
        stream.write('\n')
    for entry in schedule:
        print('Starting case:', entry['name'], flush=True)
        subprocess.run(entry['command'], cwd=ROOT, check=True)
        case = out / entry['name']
        subprocess.run([sys.executable, '-m', 'integration.upscaledb.reports.analyze', '--input-dir', str(case)],
                       cwd=ROOT, check=True)
        result = json.loads((case / 'analysis' / 'summary.json').read_text())
        if result['failure_count']:
            raise SystemExit(f"case {entry['name']} has {result['failure_count']} failed raw trials; "
                             'evidence retained, remaining cases not started')
        print('Completed case:', entry['name'], flush=True)
    return 0


if __name__ == '__main__':
    sys.exit(main())
