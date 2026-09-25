#!/usr/bin/env python3
"""Closed-loop real-DB boundary study; frozen binaries, never builds.

Run prepare under the shared measurement lock and database slot lock; run
collection under the EXCLUSIVE measurement lock and the same slot lock, with
the dashboard stopped. Every phase inherits taskset -c 48-55. Analysis only
reads saved observations and never launches benchmark binaries.
"""
import argparse
import csv
import datetime as dt
import json
import math
import os
from pathlib import Path
import random
import signal
import statistics
import subprocess
import sys

import run_trials as runner

HERE = Path(__file__).resolve().parent
DEFAULT_BINARY_ROOT = runner.ROOT / '.worktree/upscaledb'
VARIANTS = ('native', 'bridge_mutex', 'fc', 'fc_pq', 'uscl')
CPUS = list(range(48, 56))
CASES = [{'case': f'preload-{p}-workers-{w}', 'preload': p, 'workers': w,
          'roles': '1' if w == 1 else '4+4', 'cpus': CPUS[:w]}
         for p in (1000, 1000000) for w in (1, 8)]
METRICS = ('find_ops_s', 'insert_ops_s', 'total_ops_s', 'timed_process_cpu_s',
           'worker_cpu_s', 'process_cpu_s', 'timed_cpu_ns_per_window_response',
           'find_latency_mean_ns', 'find_latency_p99_upper_ns', 'find_latency_max_ns',
           'insert_latency_mean_ns', 'insert_latency_p99_upper_ns', 'insert_latency_max_ns',
           'writer_max_no_progress_s', 'drain_s', 'final_db_entries')
NOTES = [
    'Four closed-loop cases; five primary backends; three fresh-process repetitions '
    'per case/backend, 60 primary trials. All cases retained, no profile variants.',
    'One worker alternates find/insert (--roles 1 maps to native --workers 1). '
    'Eight workers use four dedicated finders plus four dedicated inserters. '
    'These role semantics differ: cross-worker comparisons are not strict scaling efficiency.',
    'Preload is the initial key count and read domain, not a fixed final working set. '
    'Unique inserts grow the database; final verified entry counts include drain inserts. '
    'Every trial uses 0.5s verified disposable warmup, destroys that DB, then creates '
    'and preloads a fresh DB for the 2s primary response window.',
    'Find and insert rates count responses by the common deadline; total throughput '
    'can change operation mix and is not a fixed-work speedup.',
    'Response mean, histogram p99 upper bounds and exact maxima include drain; '
    'unsupported p99 is null, not zero. Writer no-progress is the existing harness '
    'metric, not proof of starvation or idle-with-pending-work.',
    'Timed process CPU and worker CPU cover work (including drain); runner process '
    'CPU includes setup, warmup, verification and teardown. CPU ns/window response '
    'uses timed CPU divided by on-time responses, so includes drain CPU.',
    'Actual NUMA snapshots cover the whole process including libraries and stacks, '
    'not DB-only residency. CPU48 initialization and explicit node1 bind are checked. '
    '32GiB is RLIMIT_AS, not a resident-memory guarantee; no allocator changes.',
    'FC-PQ versus FC is separate from FC/FC-PQ versus native, bridge_mutex and USCL. '
    'Comparisons are paired only within identical preload/worker cases.',
    'Predeclared per-metric paired ratios: all three >1.05 mean consistent increase; '
    'all three <0.95 mean consistent decrease; otherwise mixed/small, not equivalence '
    'or significance. Higher throughput and lower CPU/latency are favorable. n=3 '
    'observed ranges are not confidence intervals; incomplete pairs yield no conclusion.',
    'Throughput cannot establish cache-miss/coherence causality. Eight-worker '
    'differences may include scheduling redistribution, operation mix and overhead; '
    'no uninstrumented service-fairness or pure-overhead attribution is made.',
    'Serial cooperative measurement locks prevent this cohort from overlapping '
    'timed runs, not unrelated host interference. No SMT siblings are used.',
]


def now():
    return dt.datetime.now(dt.timezone.utc).isoformat()


def load(path):
    return json.loads(path.read_text(), parse_constant=runner.reject_nonfinite)


def save(path, value):
    runner.atomic_new(path, value)


def sources():
    return {name: runner.digest(HERE / name) for name in
            (Path(__file__).name, 'run_trials.py', 'analyze.py', 'build.py')}


def pin():
    if not set(CPUS) <= os.sched_getaffinity(0):
        raise ValueError('all database-slot CPUs48-55 must be in inherited affinity')
    topology = runner.discover_topology()
    selected = [topology['cpus'][str(cpu)] for cpu in CPUS]
    if any(cpu['node'] != 1 for cpu in selected) or len({
            (cpu['socket'], cpu['core']) for cpu in selected}) != 8:
        raise ValueError('CPUs48-55 must be distinct physical cores on node1')
    os.sched_setaffinity(0, CPUS)
    if set(os.sched_getaffinity(0)) != set(CPUS):
        raise ValueError('database slot affinity not enforced')
    return topology


def artifacts(root):
    built = {}
    for variant in VARIANTS:
        binary = root / ('upscaledb-' + variant)
        if not binary.is_file() or not os.access(binary, os.X_OK):
            raise ValueError(f'frozen executable unavailable: {binary}')
        built[variant] = runner.build_provenance(
            variant, binary, root / ('build-' + variant + '.json'))
    return built


def command(root, cell, output, seed, smoke=False):
    cmd = [sys.executable, str(HERE / 'run_trials.py'), '--variants', ','.join(VARIANTS),
           '--mode', 'duration', '--roles', cell['roles'], '--cpus',
           ','.join(map(str, cell['cpus'])), '--layout', 'packed', '--exclusive-cpus',
           '--init-cpu', '48', '--init-node', '1', '--memory-policy', 'bind',
           '--memory-nodes', '1', '--memory-limit-gib', '32', '--warmup', '0.5',
           '--seconds', '0.1' if smoke else '2', '--preload', str(cell['preload']),
           '--repetitions', '1', '--seed', '1', '--order-seed', str(seed),
           '--timeout-seconds', '180', '--output-dir', str(output)]
    if smoke:
        cmd.append('--smoke')
    for variant in VARIANTS:
        cmd += ['--binary', f'{variant}={root / ("upscaledb-" + variant)}',
                '--build-manifest', f'{variant}={root / ("build-" + variant + ".json")}']
    # No --stop-on-failure: ordinary failed trials must not select the remaining matrix.
    return cmd


def execute(cmd, log, timeout):
    event = {'command': cmd, 'cwd': str(runner.ROOT), 'started_utc': now(),
             'timeout_seconds': timeout, 'affinity': sorted(os.sched_getaffinity(0))}
    with log.open('xb') as stream:
        child = subprocess.Popen(cmd, stdout=stream, stderr=subprocess.STDOUT,
                                 start_new_session=True, cwd=runner.ROOT)
        try:
            event['returncode'] = child.wait(timeout=timeout)
        except (subprocess.TimeoutExpired, KeyboardInterrupt):
            # Runner handles SIGTERM and owns cleanup of its separate binary session.
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=30)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
            event.update(returncode=child.returncode, interrupted_or_timeout=True)
        event['finished_utc'] = now()
    save(log.with_suffix('.event.json'), event)
    if event['returncode'] != 0 or event.get('interrupted_or_timeout'):
        raise RuntimeError(f'command failed; log preserved: {log}')


def analyze_cell(raw, output):
    execute([sys.executable, str(HERE / 'analyze.py'), '--input-dir', str(raw),
             '--output-dir', str(output), '--no-plots'],
            output.parent / (output.name + '.log'), 180)
    return load(output / 'summary.json')


def prepare(args):
    topology = pin()
    # Reserve a fresh root even if provenance or smoke fails, retaining gate failures.
    args.output_root.mkdir(parents=True, exist_ok=False)
    try:
        built = artifacts(args.binary_root)
        rng = random.Random(args.order_seed)
        cells = []
        for repetition in range(3):
            cases = CASES.copy()
            rng.shuffle(cases)
            for case in cases:
                name = f'rep-{repetition:02d}-{case["case"]}'
                seed = rng.randrange(2**31)
                order = list(VARIANTS)
                random.Random(seed).shuffle(order)
                cells.append({**case, 'name': name, 'repetition': repetition,
                              'order_seed': seed, 'backend_order': order,
                              'command': command(args.binary_root, case,
                                                 args.output_root / 'trials' / name, seed)})
        plan = {'schema': 1, 'experiment': 'database_boundary', 'created_utc': now(),
                'controller_command': sys.argv, 'output_root': str(args.output_root),
                'binary_root': str(args.binary_root), 'source_sha256': sources(),
                'artifacts': built, 'topology': topology, 'controller_cpus': CPUS,
                'variants': VARIANTS, 'cases': CASES, 'repetitions': 3,
                'expected_primary_trials': 60, 'duration_seconds': 2,
                'warmup_seconds': 0.5, 'memory_limit_gib': 32,
                'init_cpu': 48, 'memory_policy': 'bind', 'memory_nodes': [1],
                'order_seed': args.order_seed, 'cells': cells, 'notes': NOTES}
        save(args.output_root / 'plan.json', plan)
        for name in plan['source_sha256']:
            (args.output_root / name).write_bytes((HERE / name).read_bytes())
        smoke = args.output_root / 'smoke'
        smoke.mkdir()
        failures = []
        for case in CASES[:2]:
            output = smoke / case['case']
            try:
                execute(command(args.binary_root, case, output, args.order_seed, smoke=True),
                        smoke / (case['case'] + '.log'), 1020)
                summary = analyze_cell(output, smoke / (case['case'] + '-analysis'))
                if summary['failure_count'] or any(
                        len(summary['trials'][v]) != 1 for v in VARIANTS):
                    raise ValueError('smoke missing/invalid trials; inspect analyzer summary')
            except (OSError, ValueError, RuntimeError) as exc:
                failures.append({'case': case['case'], 'reason': str(exc)})
        if failures:
            raise ValueError('representative 1/8-worker smoke failed: ' + json.dumps(failures))
        save(args.output_root / 'prepared.json', {'finished_utc': now(),
             'plan_sha256': runner.digest(args.output_root / 'plan.json'),
             'smoke_trials': 10, 'expected_primary_trials': 60})
    except Exception as exc:
        save(args.output_root / 'prepare-failure.json', {'created_utc': now(), 'reason': str(exc)})
        raise
    print('Prepared 60 primary trials; 10 separate small-preload smoke trials passed.')


def saved_plan(root):
    plan = load(root / 'plan.json')
    prepared = load(root / 'prepared.json')
    if prepared['plan_sha256'] != runner.digest(root / 'plan.json'):
        raise ValueError('immutable prepared plan changed')
    if plan['source_sha256'] != sources():
        raise ValueError('controller/runner/analyzer/build helper changed since preparation')
    return plan


def run(args):
    pin()
    plan = saved_plan(args.output_root)
    if plan['output_root'] != str(args.output_root):
        raise ValueError('run root differs from pre-recorded command destinations')
    current = artifacts(Path(plan['binary_root']))
    for variant in VARIANTS:
        for key in ('sha256', 'build_manifest_sha256', 'source_implementation_sha256',
                    'patched_library_sha256'):
            if current[variant][key] != plan['artifacts'][variant][key]:
                raise ValueError(f'{variant}: frozen provenance changed: {key}')
    trials = args.output_root / 'trials'
    trials.mkdir(exist_ok=False)
    save(args.output_root / 'run-start.json', {'started_utc': now(), 'command': sys.argv,
         'affinity': sorted(os.sched_getaffinity(0)), 'expected_primary_trials': 60})
    try:
        for cell in plan['cells']:
            execute(cell['command'], trials / (cell['name'] + '.log'), 1020)
    except Exception as exc:
        save(args.output_root / 'run-failure.json', {'created_utc': now(), 'reason': str(exc)})
        raise
    inventory = {str(p.relative_to(args.output_root)): runner.digest(p)
                 for p in sorted(trials.rglob('*')) if p.is_file()}
    save(args.output_root / 'run-finished.json', {'finished_utc': now(),
         'expected_primary_trials': 60, 'raw_sha256': inventory})
    print('Finished 60 primary attempts; run --analyze-only for correctness/failure counts.')


def finite(value):
    return type(value) in (int, float) and math.isfinite(value)


def spread(values):
    values = [value for value in values if finite(value)]
    return {'n': len(values), 'mean': statistics.mean(values) if values else None,
            'min': min(values) if values else None, 'max': max(values) if values else None}


def extract(trial, record):
    result = record['result']
    metrics = dict(trial['metrics'])
    for op in ('find', 'insert'):
        maxima = [w['latency_ns'][op]['max_ns'] for w in result['workers']
                  if w['latency_ns'][op]['samples']]
        metrics[op + '_latency_max_ns'] = max(maxima) if maxima else None
    metrics['final_db_entries'] = result['verification']['db_count']
    count = sum(trial['on_time'])
    cpu = metrics.get('timed_process_cpu_s')
    metrics['timed_cpu_ns_per_window_response'] = cpu * 1e9 / count if count and finite(cpu) else None
    return {'metrics': metrics, 'completed': trial['completed'], 'on_time': trial['on_time'],
            'during_drain': trial['during_drain'], 'verification': result['verification'],
            'numa_memory': result.get('numa_memory'),
            'process_resource_scope': trial['process_resource_scope'],
            'workers': [{key: w[key] for key in ('id', 'finder', 'inserter', 'cpu_requested',
                        'cpu_start', 'cpu_end', 'cpu_time_ns', 'completed_before_deadline',
                        'longest_writer_no_progress_ns')} for w in result['workers']]}


def comparisons(rows):
    pairs = [('fc_pq_vs_fc', 'fc_pq', 'fc')] + [
        ('delegation_vs_conventional', d, c) for d in ('fc', 'fc_pq')
        for c in ('native', 'bridge_mutex', 'uscl')]
    index = {(r['case'], r['variant'], r['repetition']): r for r in rows if r['status'] == 'ok'}
    result = []
    for case in CASES:
        for category, numerator, denominator in pairs:
            for metric in METRICS:
                paired = []
                for rep in range(3):
                    a = index.get((case['case'], numerator, rep), {}).get('metrics', {}).get(metric)
                    b = index.get((case['case'], denominator, rep), {}).get('metrics', {}).get(metric)
                    if finite(a) and finite(b):
                        paired.append({'repetition': rep, 'difference': a - b,
                                       'ratio': a / b if b else None})
                deltas = [p['difference'] for p in paired]
                ratios = [p['ratio'] for p in paired if finite(p['ratio'])]
                direction = ('incomplete_no_conclusion' if len(ratios) != 3 else
                             'consistent_increase' if all(r > 1.05 for r in ratios) else
                             'consistent_decrease' if all(r < 0.95 for r in ratios) else
                             'mixed_or_small_not_equivalence')
                orientation = ('higher_is_better' if metric.endswith('_ops_s') else
                               'descriptive_only' if metric == 'final_db_entries' else
                               'lower_is_better')
                effect = ('no_conclusion' if direction == 'incomplete_no_conclusion' else
                          'mixed_or_small_not_equivalence')
                if direction in ('consistent_increase', 'consistent_decrease'):
                    effect = ('descriptive_only' if orientation == 'descriptive_only' else
                              'helps' if ((direction == 'consistent_increase') ==
                                          (orientation == 'higher_is_better')) else 'hurts')
                result.append({'case': case['case'], 'category': category,
                               'numerator': numerator, 'denominator': denominator,
                               'metric': metric, 'observed_direction': direction,
                               'orientation': orientation, 'effect': effect,
                               'paired': paired, 'difference': spread(deltas),
                               'ratio': spread([p['ratio'] for p in paired])})
    return result


def plots(rows, output):
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    for filename, metrics in (
            ('throughput', ('find_ops_s', 'insert_ops_s')),
            ('cpu-progress', ('timed_process_cpu_s', 'writer_max_no_progress_s')),
            ('response-tails', ('find_latency_p99_upper_ns', 'insert_latency_p99_upper_ns',
                                'find_latency_max_ns', 'insert_latency_max_ns'))):
        fig, axes = plt.subplots(4, len(metrics), figsize=(5 * len(metrics), 12), squeeze=False)
        for i, case in enumerate(CASES):
            for j, metric in enumerate(metrics):
                ax = axes[i][j]
                for x, variant in enumerate(VARIANTS):
                    selected = [r for r in rows if r['case'] == case['case'] and
                                r['variant'] == variant and r['status'] == 'ok' and
                                finite(r['metrics'].get(metric))]
                    values = [r['metrics'][metric] for r in selected]
                    ax.scatter([x + (r['repetition'] - 1) * .08 for r in selected], values)
                    if values:
                        ax.hlines(statistics.mean(values), x - .2, x + .2, colors='black')
                ax.set_xticks(range(len(VARIANTS)), VARIANTS, rotation=20)
                ax.set_title(case['case'] + '\n' + metric)
                ax.set_ylim(bottom=0)
        fig.suptitle('Database boundary: trial dots and means; no confidence intervals\n'
                     'Missing/failed trials remain in summary, never replaced')
        fig.tight_layout(rect=(0, 0, 1, .95))
        fig.savefig(output / (filename + '.png'), dpi=150)
        plt.close(fig)


def analyze_all(args):
    plan = saved_plan(args.output_root)
    output = args.output_root / 'analysis'
    output.mkdir(exist_ok=False)
    rows, failures = [], []
    finished = args.output_root / 'run-finished.json'
    inventory = load(finished)['raw_sha256'] if finished.is_file() else {}
    if not inventory:
        failures.append({'reason': 'collection incomplete; no final raw hash inventory'})
    for cell in plan['cells']:
        raw = args.output_root / 'trials' / cell['name']
        good, reasons = {}, {}
        try:
            if inventory:
                prefix = str(raw.relative_to(args.output_root)) + '/'
                for name, sha in inventory.items():
                    if name.startswith(prefix) and runner.digest(args.output_root / name) != sha:
                        raise ValueError('saved raw artifact changed: ' + name)
            summary = analyze_cell(raw, output / cell['name'])
            cfg = summary['config']
            expected = {'variants': list(VARIANTS), 'workers': cell['workers'],
                        'finders': 1 if cell['workers'] == 1 else 4,
                        'inserters': 0 if cell['workers'] == 1 else 4,
                        'cpus': cell['cpus'], 'preload': cell['preload'],
                        'mode': 'duration', 'seconds': 2, 'warmup': 0.5,
                        'memory_limit_gib': 32, 'init_cpu': 48, 'init_node': 1,
                        'requested_memory_policy': 'bind', 'memory_nodes': [1],
                        'repetitions': 1, 'smoke': False, 'seed': 1,
                        'order_seed': cell['order_seed']}
            if any(cfg.get(key) != value for key, value in expected.items()):
                raise ValueError('case configuration differs from immutable design')
            manifest = load(raw / 'manifest.json')
            if manifest['orders'] != [cell['backend_order']]:
                raise ValueError('backend order differs from immutable plan')
            for variant in VARIANTS:
                saved = manifest['artifacts']['binaries'][variant]
                if any(saved[key] != plan['artifacts'][variant][key] for key in
                       ('sha256', 'build_manifest_sha256', 'patched_library_sha256')):
                    raise ValueError('trial artifact differs from prepared manifest: ' + variant)
            good = {v: trials[0] for v, trials in summary['trials'].items() if len(trials) == 1}
            reasons = {f['file']: f['reason'] for f in summary['raw_failures']}
        except (OSError, ValueError, RuntimeError, KeyError, TypeError) as exc:
            failures.append({'cell': cell['name'], 'reason': str(exc)})
        for position, variant in enumerate(cell['backend_order']):
            file = raw / f'block-0000.{variant}.json'
            row = {'case': cell['case'], 'preload': cell['preload'], 'workers_count': cell['workers'],
                   'roles': cell['roles'], 'repetition': cell['repetition'], 'variant': variant,
                   'position': position, 'raw_file': str(file), 'status': 'failed_or_missing',
                   'metrics': {}}
            try:
                if variant not in good:
                    raise ValueError(reasons.get(file.name, 'cell unavailable or invalid; see failures'))
                row.update(extract(good[variant], load(file)), status='ok')
            except (OSError, ValueError, KeyError, TypeError) as exc:
                row['reason'] = str(exc)
                failures.append({'cell': cell['name'], 'variant': variant, 'reason': str(exc)})
            rows.append(row)
    groups = []
    for case in CASES:
        for variant in VARIANTS:
            selected = [r for r in rows if r['case'] == case['case'] and r['variant'] == variant
                        and r['status'] == 'ok']
            metrics = sorted({key for r in selected for key in r['metrics']})
            groups.append({**case, 'variant': variant, 'expected_trials': 3,
                           'successful_trials': len(selected), 'failed_trials': 3 - len(selected),
                           'metrics': {key: spread([r['metrics'].get(key) for r in selected])
                                       for key in metrics}})
    successful = sum(r['status'] == 'ok' for r in rows)
    result = {'schema': 1, 'experiment': 'database_boundary',
              'scope': 'closed_loop_real_in_memory_db_preload_1k_1m_workers_1_8',
              'created_utc': now(), 'command': sys.argv, 'expected_trials': 60,
              'successful_trials': successful, 'failed_trials': 60 - successful,
              'plan_sha256': runner.digest(args.output_root / 'plan.json'),
              'collection_source_sha256': plan['source_sha256'], 'analysis_source_sha256': sources(),
              'raw_inventory_present': bool(inventory), 'groups': groups, 'trials': rows,
              'comparisons': comparisons(rows), 'failures': failures, 'notes': NOTES}
    save(output / 'summary.json', result)
    with (output / 'summary.csv').open('x', newline='') as stream:
        writer = csv.writer(stream)
        writer.writerow(['case', 'variant', 'repetition', 'status', 'metric', 'value'])
        for row in rows:
            for metric, value in (row['metrics'] or {'unavailable': None}).items():
                writer.writerow([row['case'], row['variant'], row['repetition'], row['status'], metric, value])
    report = ['Database boundary (exploratory n=3)',
              f'Expected: 60; successful: {successful}; failed/missing: {60 - successful}.',
              *NOTES, '', 'Per-case observed ranges (mean [min, max], n):']
    for group in groups:
        report.append(f'\n{group["case"]} / {group["variant"]}: '
                      f'{group["successful_trials"]}/3 valid')
        for key in METRICS:
            value = group['metrics'].get(key, spread([]))
            report.append(f'  {key}: {value["mean"]} [{value["min"]}, {value["max"]}], n={value["n"]}')
    report += ['', 'Within-case paired throughput contrasts (numerator / denominator):']
    for comparison in result['comparisons']:
        if comparison['metric'] in ('find_ops_s', 'insert_ops_s'):
            report.append(f'{comparison["case"]} {comparison["category"]} '
                          f'{comparison["numerator"]}/{comparison["denominator"]} '
                          f'{comparison["metric"]}: {comparison["observed_direction"]}; '
                          f'ratio {comparison["ratio"]}')
    report += ['', 'Failures:', json.dumps(failures, indent=2)]
    (output / 'report.txt').write_text('\n'.join(report) + '\n')
    try:
        plots(rows, output)
    except Exception as exc:
        save(output / 'plot-failure.json', {'created_utc': now(), 'reason': str(exc)})
        raise
    print('Analysis:', output / 'summary.json')
    if failures:
        raise ValueError('analysis retains failures/incomplete collection; inspect report.txt')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group(required=True)
    modes.add_argument('--prepare-only', action='store_true')
    modes.add_argument('--run', action='store_true')
    modes.add_argument('--analyze-only', action='store_true')
    parser.add_argument('--output-root', type=Path, required=True)
    parser.add_argument('--binary-root', type=Path, default=DEFAULT_BINARY_ROOT,
                        help='prepare-only frozen build directory; never rebuilt')
    parser.add_argument('--order-seed', type=int, default=20260924)
    args = parser.parse_args()
    args.output_root = args.output_root.expanduser().resolve()
    args.binary_root = args.binary_root.expanduser().resolve()
    if args.prepare_only:
        prepare(args)
    elif args.run:
        run(args)
    else:
        analyze_all(args)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError, KeyError, TypeError) as exc:
        print('Database boundary BLOCKED:', exc, file=sys.stderr)
        sys.exit(1)
