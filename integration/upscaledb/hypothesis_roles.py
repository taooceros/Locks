#!/usr/bin/env python3
"""H1: real UpScaleDB role-cost asymmetry; frozen binaries, never builds.

Prepare first, then run (timed collection only), then analyze separately.
All new data roots refuse reuse; analysis regenerations get new directories.
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
import statistics as stats
import subprocess
import sys
import tempfile

import analyze
import run_trials as runner

HERE = Path(__file__).resolve().parent
PRIMARY = ('fc', 'fc_pq', 'uscl')
PROFILES = tuple(v + '_profile' for v in PRIMARY)
VARIANTS = PRIMARY + PROFILES
ROLES = ('4+4', '7+1', '1+7')
CPUS = list(range(8))
PRIMARY_METRICS = (
    'find_ops_s', 'insert_ops_s', 'total_ops_s', 'timed_process_cpu_s',
    'worker_cpu_s', 'process_cpu_s', 'find_latency_mean_ns',
    'insert_latency_mean_ns', 'find_latency_p99_upper_ns',
    'insert_latency_p99_upper_ns', 'writer_max_no_progress_s', 'drain_s')
NOTES = [
    'Exploratory n=3; ranges are observed trial ranges, not confidence intervals.',
    'H1 uses CPU0-7/node0 with no SMT siblings; other hypotheses may co-run on '
    'other physical cores. Shared socket/LLC/memory interference is not eliminated.',
    'Profile results are instrumented and NEVER primary throughput evidence.',
    'Service is actual callback elapsed ns attributed to request origin, not CPU time; '
    'response-window sums credit whole callbacks whose responses finish by the common '
    '5s deadline, exclude drain, and are not clipped integrals inside the window.',
    'Window service means divide measured before-deadline service by before-deadline '
    'responses. Full histogram service means include drain and are reported separately.',
    'Equal-weight descriptive role reference is role workers/8 (worker reference 1/8). '
    'Workers continuously reissue, but finite reissue gaps mean this is not a strict '
    'entitlement proof; no pending-request timeline is available.',
    'Service Jain uses all eight requester service totals, not operation throughput. '
    'Within-role Jain for a singleton is undefined (null), not a fabricated score.',
    'Latency histograms include drain; quantiles are histogram upper bounds. Writer '
    'gaps are the harness longest-no-progress metric, not inferred queueing delay.',
    'Timed process CPU and worker CPU cover work; runner process CPU includes '
    'setup/warmup/verification/teardown. They are kept separate.',
    'Cost-asymmetry screening threshold is a predeclared 1.2x service-mean ratio; '
    'all three repetitions must agree in direction. This is descriptive, not significance.',
    'FC-PQ allocation improves a comparator only if all three paired repetitions '
    'increase all-worker service Jain AND reduce absolute finder-share error. '
    'Mixed/equal deltas are inconclusive; no favorable outcome is presumed.',
]


def now():
    return dt.datetime.now(dt.timezone.utc).isoformat()


def load(path):
    return json.loads(path.read_text(), parse_constant=runner.reject_nonfinite)


def save(path, value):
    runner.atomic_new(path, value)


def sources():
    return {p.name: runner.digest(p) for p in
            (Path(__file__), HERE / 'run_trials.py', HERE / 'analyze.py')}


def fresh(path):
    path.mkdir(parents=True, exist_ok=False)


def pin():
    if not set(CPUS) <= os.sched_getaffinity(0):
        raise ValueError('H1 requires all CPUs 0-7 in the inherited affinity mask')
    topology = runner.discover_topology()
    selected = [topology['cpus'][str(c)] for c in CPUS]
    if any(c['node'] != 0 for c in selected) or len({
            (c['socket'], c['core']) for c in selected}) != 8:
        raise ValueError('H1 CPUs must be eight distinct physical cores on node0')
    os.sched_setaffinity(0, CPUS)
    if set(os.sched_getaffinity(0)) != set(CPUS):
        raise ValueError('H1 controller affinity was not enforced')
    return topology


def artifacts(build_root):
    result = {}
    for v in VARIANTS:
        binary = build_root / ('upscaledb-' + v)
        if not binary.is_file() or not os.access(binary, os.X_OK):
            raise ValueError(f'{v}: frozen executable unavailable: {binary}')
        result[v] = runner.build_provenance(v, binary, build_root / ('build-' + v + '.json'))
    return result


def command(args, role, output, seed, smoke=False):
    cmd = [sys.executable, str(HERE / 'run_trials.py'), '--variants', ','.join(VARIANTS),
           '--mode', 'duration', '--roles', role, '--cpus', '0,1,2,3,4,5,6,7',
           '--layout', 'packed', '--exclusive-cpus', '--init-cpu', '0', '--init-node', '0',
           '--memory-policy', 'bind', '--memory-nodes', '0', '--memory-limit-gib', '32',
           '--warmup', '0.05' if smoke else '0.5', '--seconds', '0.1' if smoke else '5',
           '--preload', '1000' if smoke else '100000', '--repetitions', '1',
           '--seed', '1', '--order-seed', str(seed), '--timeout-seconds', '120',
           '--stop-on-failure', '--output-dir', str(output)]
    if smoke:
        cmd.append('--smoke')
    for v in VARIANTS:
        cmd += ['--binary', f'{v}={args.build_root / ("upscaledb-" + v)}',
                '--build-manifest', f'{v}={args.build_root / ("build-" + v + ".json")}']
    return cmd


def execute(cmd, log, timeout):
    event = {'command': cmd, 'started_utc': now(), 'timeout_seconds': timeout}
    # The runner owns a separate child session: let its SIGTERM handler clean up.
    with log.open('xb') as stream:
        child = subprocess.Popen(cmd, stdout=stream, stderr=subprocess.STDOUT,
                                 start_new_session=True, cwd=runner.ROOT)
        try:
            event['returncode'] = child.wait(timeout=timeout)
        except (subprocess.TimeoutExpired, KeyboardInterrupt):
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=20)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
            event.update(returncode=child.returncode, interrupted_or_timeout=True)
        event['finished_utc'] = now()
    save(log.with_suffix('.event.json'), event)
    if event['returncode'] != 0 or event.get('interrupted_or_timeout'):
        raise RuntimeError(f'command failed; preserved log: {log}')


def analyze_cell(raw, output):
    execute([sys.executable, str(HERE / 'analyze.py'), '--input-dir', str(raw),
             '--output-dir', str(output), '--no-plots'], output.parent / (output.name + '.log'), 120)
    summary = load(output / 'summary.json')
    if summary['failure_count']:
        raise ValueError(f'{raw}: {summary["failure_count"]} invalid/missing trials; see {output}')
    return summary


def prepare(args):
    topology = pin()
    built = artifacts(args.build_root)
    fresh(args.output_root)
    smoke = args.smoke_root or args.output_root.with_name(args.output_root.name + '-smoke')
    fresh(smoke)
    rng = random.Random(args.order_seed)
    cells = []
    for rep in range(3):
        roles = list(args.roles)
        rng.shuffle(roles)
        for role in roles:
            name = f'rep-{rep:02d}-roles-{role}'
            cells.append({'name': name, 'repetition': rep, 'roles': role,
                          'command': command(args, role, args.output_root / 'trials' / name,
                                             rng.randrange(2**31))})
    plan = {'schema': 1, 'hypothesis': 'H1_role_cost_service_allocation',
            'created_utc': now(), 'co_run_label': args.co_run_label,
            'build_root': str(args.build_root), 'output_root': str(args.output_root),
            'smoke_root': str(smoke), 'controller_command': sys.argv,
            'source_sha256': sources(), 'artifacts': built, 'topology': topology,
            'cpus': CPUS, 'init_cpu': 0, 'memory_policy': 'bind', 'memory_nodes': [0],
            'memory_limit_gib': 32, 'duration_seconds': 5, 'warmup_seconds': 0.5,
            'preload': 100000, 'primary_variants': PRIMARY, 'profile_variants': PROFILES,
            'roles': args.roles, 'repetitions': 3, 'planned_trials': len(cells) * len(VARIANTS),
            'order_seed': args.order_seed, 'cells': cells, 'notes': NOTES}
    save(args.output_root / 'plan.json', plan)
    # Keep an exact controller/helper copy even if the working tree later changes.
    for filename in plan['source_sha256']:
        (args.output_root / filename).write_bytes((HERE / filename).read_bytes())
    execute(command(args, '4+4', smoke / 'raw', args.order_seed, smoke=True),
            smoke / 'runner.log', 780)
    smoke_summary = analyze_cell(smoke / 'raw', smoke / 'analysis')
    for v in VARIANTS:
        if len(smoke_summary['trials'][v]) != 1:
            raise ValueError(f'smoke missing variant {v}')
        extract(smoke_summary['trials'][v][0], '4+4', 0, smoke=True)
    save(args.output_root / 'prepared.json', {'schema': 1, 'finished_utc': now(),
                                            'plan_sha256': runner.digest(args.output_root / 'plan.json'),
                                            'smoke_root': str(smoke)})
    print(f'Prepared {plan["planned_trials"]} timed trials; smoke is separate:', smoke)


def run(args):
    pin()
    plan = load(args.output_root / 'plan.json')
    prepared = load(args.output_root / 'prepared.json')
    if prepared['plan_sha256'] != runner.digest(args.output_root / 'plan.json'):
        raise ValueError('prepared plan changed')
    if plan['source_sha256'] != sources():
        raise ValueError('controller/runner/analyzer changed since preparation')
    current = artifacts(Path(plan['build_root']))
    for v in VARIANTS:
        for key in ('sha256', 'build_manifest_sha256', 'source_implementation_sha256',
                    'patched_library_sha256'):
            if current[v][key] != plan['artifacts'][v][key]:
                raise ValueError(f'{v}: frozen provenance changed since preparation: {key}')
    fresh(args.output_root / 'trials')
    save(args.output_root / 'run-start.json', {'started_utc': now(), 'command': sys.argv,
         'co_run_label': plan['co_run_label'], 'source_sha256': sources(), 'cpus': CPUS})
    for cell in plan['cells']:
        execute(cell['command'], args.output_root / 'trials' / (cell['name'] + '.log'), 780)
    save(args.output_root / 'run-finished.json',
         {'finished_utc': now(), 'planned_trials': plan['planned_trials']})
    print(f'Collected {plan["planned_trials"]} trials serially inside H1; analyze separately.')


def number(value, label):
    if not isinstance(value, (float, int)) or not math.isfinite(value):
        raise ValueError('missing/nonfinite metric: ' + label)
    return value


def extract(trial, role, rep, smoke=False):
    v = trial['variant']
    m = trial['metrics']
    row = {'roles': role, 'repetition': rep, 'variant': v,
           'kind': 'profile' if v in PROFILES else 'primary',
           'metrics': {k: number(m.get(k), k) for k in PRIMARY_METRICS
                       if not (smoke and '_p99_' in k)}}
    if v not in PROFILES:
        return row
    f, ins = map(int, role.split('+'))
    service = trial['bridge_profile']['before_deadline_ns']
    if not trial['bridge_profile']['instrumented'] or len(service) != 8:
        raise ValueError('missing eight instrumented requester service observations')
    amounts = [sum(number(x, 'worker service') for x in pair) for pair in service]
    total = sum(amounts)
    if total <= 0:
        raise ValueError('no measured requester service in response window')
    metrics = row['metrics']
    metrics.update(service_total_ns=total, service_jfi=analyze.jain(amounts),
                   finder_share=sum(amounts[:f]) / total,
                   inserter_share=sum(amounts[f:]) / total,
                   finder_reference=f / 8, inserter_reference=ins / 8,
                   finder_service_jfi=analyze.jain(amounts[:f]),
                   inserter_service_jfi=analyze.jain(amounts[f:]))
    metrics['role_share_error'] = abs(metrics['finder_share'] - f / 8)
    for idx, op in enumerate(('find', 'insert')):
        count = trial['on_time'][idx]
        if count <= 0:
            raise ValueError('no before-deadline responses for ' + op)
        metrics[op + '_window_service_mean_ns'] = sum(s[idx] for s in service) / count
        metrics[op + '_full_service_mean_ns'] = number(
            m.get(op + '_requester_service_mean_ns'), op + ' full service mean')
    if metrics['find_window_service_mean_ns'] <= 0:
        raise ValueError('nonpositive measured find service')
    metrics['insert_find_service_ratio'] = (metrics['insert_window_service_mean_ns'] /
                                             metrics['find_window_service_mean_ns'])
    row['workers'] = [{'worker': i, 'role': 'finder' if i < f else 'inserter',
                       'service_ns_by_operation': service[i], 'service_ns': amount,
                       'service_share': amount / total, 'reference': 1 / 8,
                       'responses_by_operation': trial['per_worker_before_deadline'][i]}
                      for i, amount in enumerate(amounts)]
    return row


def spread(values):
    return {'mean': stats.mean(values), 'min': min(values), 'max': max(values), 'n': len(values)}


def decisions(rows):
    index = {(r['roles'], r['variant'], r['repetition']): r['metrics'] for r in rows}
    cost, allocation, primary, overhead = [], [], [], []
    for role in dict.fromkeys(r['roles'] for r in rows):
        for v in PROFILES:
            ratios = [index[role, v, rep]['insert_find_service_ratio'] for rep in range(3)]
            verdict = ('yes_insert_costlier' if min(ratios) > 1.2 else
                       'yes_find_costlier' if max(ratios) < 1 / 1.2 else
                       'no_material_asymmetry_at_1.2x' if min(ratios) >= 1 / 1.2 and
                       max(ratios) <= 1.2 else 'inconclusive')
            cost.append({'roles': role, 'variant': v, 'decision': verdict,
                         'insert_find_window_service_ratios': ratios})
        for comparator in ('fc', 'uscl'):
            jfi, error = [], []
            for rep in range(3):
                pq = index[role, 'fc_pq_profile', rep]
                base = index[role, comparator + '_profile', rep]
                jfi.append(pq['service_jfi'] - base['service_jfi'])
                error.append(base['role_share_error'] - pq['role_share_error'])
            verdict = ('yes' if min(jfi) > 0 and min(error) > 0 else
                       'no' if max(jfi) <= 0 and max(error) <= 0 else 'inconclusive')
            allocation.append({'roles': role, 'comparator': comparator, 'decision': verdict,
                               'paired_jfi_increases': jfi, 'paired_share_error_reductions': error})
            for metric in PRIMARY_METRICS:
                a = [index[role, 'fc_pq', r][metric] for r in range(3)]
                b = [index[role, comparator, r][metric] for r in range(3)]
                primary.append({'roles': role, 'comparator': comparator, 'metric': metric,
                                'fc_pq': spread(a), 'baseline': spread(b),
                                'paired_differences': [x - y for x, y in zip(a, b)],
                                'ratio_of_means': stats.mean(a) / stats.mean(b) if any(b) else None})
        for v in PRIMARY:
            for metric in PRIMARY_METRICS:
                a = [index[role, v + '_profile', r][metric] for r in range(3)]
                b = [index[role, v, r][metric] for r in range(3)]
                overhead.append({'roles': role, 'variant': v, 'metric': metric,
                                 'profile': spread(a), 'primary': spread(b),
                                 'profile_primary_ratio_of_means':
                                 stats.mean(a) / stats.mean(b) if any(b) else None})
    verdicts = [x['decision'] for x in allocation]
    cost_verdicts = [x['decision'] for x in cost]
    return {'cost_asymmetry': cost,
            'cost_asymmetry_overall': ('yes_across_all_profile_cells' if all(
                v.startswith('yes_') for v in cost_verdicts) else
                'no_material_asymmetry_at_1.2x' if all(v.startswith('no_') for v in cost_verdicts)
                else 'conditional_or_inconclusive_see_cells'),
            'fc_pq_allocation': allocation,
            'fc_pq_improves_both_baselines_across_roles': (
                'yes' if all(v == 'yes' for v in verdicts) else
                'no' if all(v == 'no' for v in verdicts) else 'conditional_or_inconclusive_see_cells'),
            'primary_comparisons': primary, 'instrumentation_comparisons': overhead}


def plots(rows, output):
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    roles = list(dict.fromkeys(r['roles'] for r in rows))
    for filename, variants, fields, title in (
        ('profile-service.png', PROFILES, ('service_jfi', 'finder_share', 'inserter_share'),
         'INSTRUMENTED requester service: common 5s response window'),
        ('primary-throughput.png', PRIMARY, ('find_ops_s', 'insert_ops_s'),
         'PRIMARY uninstrumented operation throughput (responses / 5s)')):
        fig, axes = plt.subplots(len(roles), len(fields),
                                 figsize=(5 * len(fields), 3.5 * len(roles)), squeeze=False)
        for i, role in enumerate(roles):
            for j, field in enumerate(fields):
                ax = axes[i][j]
                for x, v in enumerate(variants):
                    values = [r['metrics'][field] for r in rows if r['roles'] == role and r['variant'] == v]
                    ax.scatter([x - .08, x, x + .08], values, s=24)
                    ax.plot([x - .2, x + .2], [stats.mean(values)] * 2, color='black')
                if 'share' in field:
                    f, ins = map(int, role.split('+'))
                    ax.axhline((f if field == 'finder_share' else ins) / 8,
                               color='gray', linestyle='--', label='workers / 8 reference')
                    ax.legend(fontsize=8)
                if variants == PROFILES:
                    ax.set_ylim(0, 1.05)
                ax.set_xticks(range(len(variants)), variants, rotation=15)
                ax.set_title(role + ' | ' + field)
                ax.set_ylabel('service fraction / JFI' if variants == PROFILES else 'operations / second')
        fig.suptitle(title + '\nDots: independent trials; black line: mean; no confidence intervals')
        fig.tight_layout(rect=(0, 0, 1, .94))
        fig.savefig(output / filename, dpi=160)
        plt.close(fig)


def analyze_all(args):
    plan = load(args.output_root / 'plan.json')
    directory = args.output_root / 'analysis'
    directory.mkdir(exist_ok=True)
    output = Path(tempfile.mkdtemp(prefix='generation-', dir=directory))
    rows, failures = [], []
    for cell in plan['cells']:
        try:
            summary = analyze_cell(args.output_root / 'trials' / cell['name'], output / cell['name'])
            f, ins = map(int, cell['roles'].split('+'))
            expected = {'finders': f, 'inserters': ins, 'workers': 8, 'cpus': CPUS,
                        'mode': 'duration', 'seconds': 5, 'warmup': 0.5, 'preload': 100000,
                        'memory_limit_gib': 32, 'init_cpu': 0, 'init_node': 0,
                        'requested_memory_policy': 'bind', 'memory_nodes': [0],
                        'variants': list(VARIANTS), 'repetitions': 1, 'smoke': False}
            if any(summary['config'].get(k) != v for k, v in expected.items()):
                raise ValueError('cell configuration differs from H1 design')
            for v in VARIANTS:
                trials = summary['trials'][v]
                if len(trials) != 1:
                    raise ValueError(f'{v}: expected exactly one trial in cell')
                rows.append(extract(trials[0], cell['roles'], cell['repetition']))
        except (ValueError, KeyError, TypeError, OSError, RuntimeError) as exc:
            failures.append({'cell': cell['name'], 'reason': str(exc)})
    if len(rows) != plan['planned_trials']:
        failures.append({'reason': f'expected {plan["planned_trials"]} valid trials, obtained {len(rows)}'})
    result = {'schema': 1, 'hypothesis': 'H1', 'created_utc': now(), 'command': sys.argv,
              'plan_sha256': runner.digest(args.output_root / 'plan.json'),
              'collection_source_sha256': plan['source_sha256'], 'analysis_source_sha256': sources(),
              'co_run_label': plan['co_run_label'], 'planned_trials': plan['planned_trials'],
              'valid_trials': len(rows), 'failures': failures, 'notes': NOTES, 'trials': rows,
              'decisions': decisions(rows) if not failures else {'status': 'blocked_missing_or_invalid_metrics'}}
    save(output / 'summary.json', result)
    with (output / 'summary.csv').open('x', newline='') as stream:
        writer = csv.writer(stream)
        writer.writerow(['roles', 'repetition', 'variant', 'kind', 'metric', 'value'])
        for row in rows:
            for metric, value in row['metrics'].items():
                writer.writerow([row['roles'], row['repetition'], row['variant'], row['kind'], metric, value])
    with (output / 'workers.csv').open('x', newline='') as stream:
        writer = csv.writer(stream)
        writer.writerow(['roles', 'repetition', 'variant', 'worker', 'role', 'service_ns', 'service_share', 'reference'])
        for row in rows:
            for worker in row.get('workers', []):
                writer.writerow([row['roles'], row['repetition'], row['variant']] +
                                [worker[k] for k in ('worker', 'role', 'service_ns', 'service_share', 'reference')])
    report = ['# H1: role cost and service allocation', '',
              f'Valid trials: {len(rows)}/{plan["planned_trials"]}. Co-run label: {plan["co_run_label"]}.',
              f'Selected role cases: {plan["roles"]}; conclusions cover only these cases.', '',
              '## Scope and decision rules', *['- ' + note for note in NOTES], '',
              '## Decisions', '```json', json.dumps({k: v for k, v in result['decisions'].items()
                  if k not in ('primary_comparisons', 'instrumentation_comparisons')}, indent=2),
              '```', '',
              'Primary CPU, throughput, latency and gap comparisons, including observed ranges, '
              'paired differences and ratios, are in summary.json / decisions / primary_comparisons. '
              'Instrumentation comparisons are separate under instrumentation_comparisons.', '',
              '## Failures', '```json', json.dumps(failures, indent=2), '```', '',
              'Full trial metrics: summary.csv; profiled worker shares: workers.csv. '
              'Each cell retains the existing analyzer reports and original raw JSON is unchanged.']
    (output / 'report.md').write_text('\n'.join(report) + '\n')
    if failures:
        raise ValueError(f'H1 analysis BLOCKED; failures preserved in {output / "summary.json"}')
    try:
        plots(rows, output)
    except Exception as exc:
        save(output / 'plot-failure.json', {'error': str(exc), 'created_utc': now()})
        raise
    print('H1 report:', output / 'report.md')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group(required=True)
    modes.add_argument('--prepare-only', action='store_true')
    modes.add_argument('--run', action='store_true')
    modes.add_argument('--analyze-only', action='store_true')
    parser.add_argument('--output-root', type=Path, required=True)
    parser.add_argument('--smoke-root', type=Path)
    parser.add_argument('--build-root', type=Path,
                        default=runner.ROOT / '.worktree/upscaledb-joined-build')
    parser.add_argument('--co-run-label', default='H1-with-other-hypotheses-disjoint-physical-cores')
    parser.add_argument('--order-seed', type=int, default=20260924)
    parser.add_argument('--roles', nargs='+', choices=ROLES, default=list(ROLES),
                        help='prepare-only case filter; e.g. --roles 4+4 (18 timed trials)')
    args = parser.parse_args()
    if len(args.roles) != len(set(args.roles)):
        parser.error('--roles must not contain duplicate cases')
    if not args.prepare_only and args.roles != list(ROLES):
        parser.error('--roles is prepare-only; run/analyze use the immutable saved selection')
    args.output_root = args.output_root.expanduser().resolve()
    args.build_root = args.build_root.expanduser().resolve()
    if args.smoke_root:
        args.smoke_root = args.smoke_root.expanduser().resolve()
    if args.prepare_only:
        prepare(args)
    elif args.run:
        run(args)
    else:
        analyze_all(args)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, RuntimeError, OSError, KeyError, TypeError) as exc:
        print('H1 FAILED:', exc, file=sys.stderr)
        sys.exit(1)
