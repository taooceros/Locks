#!/usr/bin/env python3
"""Reduce three completed cohorts into tables and portable, evidence-linked HTML."""
import argparse
import hashlib
import json
from pathlib import Path
import statistics
from fairness_report_view import render_report


def load(path):
    return json.loads(path.read_text())


def spread(values):
    values = [v for v in values if v is not None]
    return {'n': len(values), 'median': statistics.median(values) if values else None,
            'min': min(values) if values else None, 'max': max(values) if values else None,
            'points': values}


def paired(rows, backend, reference, field, rep='rep'):
    a = {r[rep]: r for r in rows if r['backend'] == backend}
    b = {r[rep]: r for r in rows if r['backend'] == reference}
    return spread([a[k][field] / b[k][field] for k in sorted(a.keys() & b.keys())
                   if b[k][field]])


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--contention', type=Path, required=True, help='completed analysis directory')
    p.add_argument('--heterogeneous', type=Path, required=True, help='raw root with analysis')
    p.add_argument('--redb', type=Path, required=True, help='raw root with analysis')
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--findings', type=Path, help='final Chinese interpretation, included verbatim')
    a = p.parse_args()
    inputs = {'contention': a.contention / 'summary.json',
              'heterogeneous': a.heterogeneous / 'analysis/summary.json',
              'redb': a.redb / 'analysis/summary.json'}
    c, h, r = (load(inputs[k]) for k in inputs)
    if c['primary']['ok'] != 450 or c['diagnostics']['ok'] != 27:
        raise ValueError('contention cohort incomplete; inspect original failures')
    if h['successful_trials'] != 252 or h['failed_trials'] or h['missing_trials']:
        raise ValueError('heterogeneous cohort incomplete; inspect original failures')
    if len(r['rows']) != 180 or r['failures']:
        raise ValueError('redb cohort incomplete; inspect original failures')
    out = a.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    result = {'sources': {k: {'path': str(v.resolve()), 'sha256': hashlib.sha256(v.read_bytes()).hexdigest()}
                          for k, v in inputs.items()},
              'counts': {'contention_primary': 450, 'contention_diagnostics': 27,
                         'heterogeneous_primary': 126, 'heterogeneous_profile': 126,
                         'redb_primary': 90, 'redb_profile': 90},
              'contention': [], 'heterogeneous': [], 'redb': [],
              'heterogeneous_pairs': [], 'redb_pairs': []}
    for workers in (8, 16, 32):
        for routing in ('uniform', 'hot90', 'hot100'):
            row = {'workers': workers, 'routing': routing}
            for reference in ('mcs', 'fc'):
                matches = [x for x in c['contrasts'] if x['role'] == 'total' and
                           x['metric'] == 'throughput_ops_s' and
                           x['numerator'] == f'fc_pq/{workers}/{routing}' and
                           x['reference'] == f'{reference}/{workers}/{routing}']
                if len(matches) != 1:
                    raise ValueError('ambiguous contention comparison')
                row['pq_vs_' + reference] = spread(matches[0]['ratio']['points'])
            for backend in ('fc', 'fc_pq', 'mcs'):
                row[backend + '_ops_s'] = spread([x['throughput_ops_s'] for x in c['measurements']
                    if (x['workers'], x['routing'], x['backend'], x['role']) ==
                    (workers, routing, backend, 'total')])
            result['contention'].append(row)
    for mix in ('reads', 'single', 'batch'):
        reader_count = 8 if mix == 'reads' else 4
        for place in ('shared', 'split'):
            group = [x for x in h['rows'] if (x['mix'], x['placement']) == (mix, place)]
            for backend in ('native', 'bridge_mutex', 'fc', 'fc_pq', 'uscl', 'cfl_local', 'mcs'):
                primary = [x['metrics'] for x in group if x['backend'] == backend and not x['profile']]
                profile = [x['metrics'] for x in group if x['backend'] == backend and x['profile']]
                row = {'mix': mix, 'placement': place, 'backend': backend}
                for key in ('reads_per_s', 'inserted_records_per_s', 'total_requests_per_s',
                            'cpu_seconds_per_second', 'max_requester_no_completion_gap_ns',
                            'zero_progress_client_windows'):
                    row[key] = spread([x[key] for x in primary])
                row['worst_reader_p95_ns'] = spread([max((n for n in x['latency_p95_upper_ns'][:reader_count]
                                                        if n is not None), default=None) for x in primary])
                row['service_jain_by_domain'] = [spread([x['domains'][i]['jain_service'] for x in profile])
                                                 for i in range(1 if place == 'shared' else 2)]
                row['profile_reader_service_share'] = (spread([sum(x['domains'][0]['service_shares'][:reader_count])
                    for x in profile if x['domains'][0]['service_shares']]) if place == 'shared' else None)
                result['heterogeneous'].append(row)
            primary = [dict(x['metrics'], backend=x['backend'], rep=x['rep']) for x in group if not x['profile']]
            result['heterogeneous_pairs'].append(dict(mix=mix, placement=place,
                **{key: paired(primary, 'fc_pq', 'fc', key) for key in
                   ('reads_per_s', 'inserted_records_per_s', 'total_requests_per_s', 'cpu_seconds_per_second')}))
    for cohort in ('all1', 'half1_half8', 'half1_half64'):
        for durability in ('immediate', 'none'):
            group = [x for x in r['rows'] if (x['cohort'], x['durability']) == (cohort, durability)]
            for backend in ('native', 'mutex', 'mcs', 'fc', 'fc_pq'):
                primary = [x for x in group if x['backend'] == backend and x['build'] == 'primary']
                profile = [x for x in group if x['backend'] == backend and x['build'] == 'profile']
                row = {'cohort': cohort, 'durability': durability, 'backend': backend}
                for key in ('throughput_records_s', 'throughput_tx_s', 'short_tx_s', 'long_tx_s',
                            'process_cpu_seconds', 'short_response_p99_ms_upper',
                            'max_zero_progress_windows'):
                    row[key] = spread([x[key] for x in primary])
                row['service_wall_jain'] = spread([x['service_wall_jain'] for x in profile])
                row['first_four_service_share'] = spread([sum(x['worker_service_wall_seconds'][:4]) /
                    sum(x['worker_service_wall_seconds']) for x in profile if sum(x['worker_service_wall_seconds'])])
                result['redb'].append(row)
            primary = [x for x in group if x['build'] == 'primary']
            result['redb_pairs'].append(dict(cohort=cohort, durability=durability,
                **{key: paired(primary, 'fc_pq', 'fc', key, 'repeat') for key in
                   ('throughput_records_s', 'throughput_tx_s', 'short_tx_s', 'process_cpu_seconds')}))
    result['profile_perturbation'] = {'heterogeneous': h['profile_perturbation_paired'],
                                    'redb': r['profile_perturbation'],
                                    'contention': c['diagnostics']['perturbations']}
    (out / 'reduced.json').write_text(json.dumps(result, indent=2, ensure_ascii=False) + '\n')

    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    colors = {'native': '#444444', 'mutex': '#a77b00', 'mcs': '#2873ad', 'fc': '#19804c', 'fc_pq': '#bd3154'}
    figures = []
    def save(fig, name):
        fig.tight_layout()
        fig.savefig(out / (name + '.png'), dpi=150)
        plt.close(fig)
        figures.append(name)
    fig, axes = plt.subplots(1, 2, figsize=(12, 4))
    for ax, reference in zip(axes, ('mcs', 'fc')):
        for offset, routing in enumerate(('uniform', 'hot90', 'hot100')):
            cells = [x for x in result['contention'] if x['routing'] == routing]
            line, = ax.plot([x['workers'] for x in cells], [x['pq_vs_' + reference]['median'] for x in cells],
                            marker='o', label=routing)
            for cell in cells:
                points = cell['pq_vs_' + reference]['points']
                ax.scatter([cell['workers'] + (i-2)*.18 for i in range(len(points))], points,
                           color=line.get_color(), alpha=.45, s=15)
        ax.axhline(1, color='gray', linestyle='--')
        ax.set(xlabel='Physical workers', ylabel=f'FC-PQ / {reference.upper()} throughput', xticks=(8, 16, 32))
        ax.legend(); ax.grid(alpha=.2)
    save(fig, 'contention-frontier')
    fig, axes = plt.subplots(1, 2, figsize=(12, 4))
    for ax, mix in zip(axes, ('single', 'batch')):
        for backend in ('mcs', 'fc', 'fc_pq'):
            values = [x['metrics']['domains'][0]['service_shares'] for x in h['rows']
                      if (x['mix'], x['placement'], x['backend'], x['profile']) == (mix, 'shared', backend, True)]
            means = [statistics.mean(v[i] for v in values) for i in range(8)]
            ax.plot(range(8), means, marker='o', color=colors[backend], label=backend)
            for v in values:
                ax.scatter(range(8), v, color=colors[backend], alpha=.3, s=12)
        ax.axhline(1/8, color='gray', linestyle='--')
        ax.set(title=f'UpScaleDB {mix}, shared environment (profile)', xlabel='Requester (0–3 readers, 4–7 writers)',
               ylabel='Share of measured serialized service', ylim=(0, 1))
        ax.legend(); ax.grid(alpha=.2)
    save(fig, 'heterogeneous-service')
    fig, axes = plt.subplots(2, 3, figsize=(14, 8))
    for col, cohort in enumerate(('all1', 'half1_half8', 'half1_half64')):
        for row, durability in enumerate(('immediate', 'none')):
            ax = axes[row, col]
            for backend in colors:
                vals = [x for x in r['rows'] if (x['cohort'], x['durability'], x['build'], x['backend']) ==
                        (cohort, durability, 'profile', backend)]
                ax.scatter([x['service_wall_jain'] for x in vals], [x['throughput_records_s'] for x in vals],
                           label=backend, color=colors[backend], alpha=.7)
            ax.set(title=f'{cohort} / {durability}', xlabel='Protected service-wall Jain (profile)', ylabel='Records/s (profile)', xlim=(0,1.02))
            ax.grid(alpha=.2)
    axes[0,0].legend(fontsize=8)
    save(fig, 'redb-fairness-frontier')
    fig, axes = plt.subplots(2, 3, figsize=(14, 7))
    for col, cohort in enumerate(('all1', 'half1_half8', 'half1_half64')):
        for row, durability in enumerate(('immediate', 'none')):
            ax = axes[row,col]
            for index, backend in enumerate(colors):
                vals = [x['process_cpu_seconds']/2 for x in r['rows'] if
                        (x['cohort'],x['durability'],x['build'],x['backend']) == (cohort,durability,'primary',backend)]
                ax.scatter([index]*len(vals), vals, color=colors[backend])
            ax.set(title=f'{cohort} / {durability}', ylabel='Process CPU seconds / window second',
                   xticks=range(len(colors)), xticklabels=list(colors), ylim=(0,None))
            ax.grid(alpha=.2)
    save(fig, 'redb-cpu-cost')

    text = a.findings.read_text() if a.findings else ''
    if text:
        (out / 'report.txt').write_text(text + '\n\n数值附表：reduced.json；图表与完整数值表：report.html。\n')
    render_report(out, result, figures, text)
    print(json.dumps({'counts': result['counts'], 'output': str(out), 'figures': figures}))


if __name__ == '__main__':
    main()
