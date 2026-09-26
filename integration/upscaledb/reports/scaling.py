#!/usr/bin/env python3
"""Aggregate completed fast-study cases without rerunning or pooling experiments."""
import argparse
import csv
import hashlib
import json
import math
import statistics
from pathlib import Path

VARIANTS = ('native', 'fc', 'fc_pq', 'uscl', 'cfl_local')
LABELS = {'native': 'Native', 'fc': 'FC', 'fc_pq': 'FC-PQ',
          'uscl': 'USCL', 'cfl_local': 'CFL local'}
METRICS = ('elapsed_s', 'find_ops_s', 'insert_ops_s', 'total_ops_s',
           'timed_process_cpu_s', 'writer_max_no_progress_s',
           'find_latency_mean_ns', 'insert_latency_mean_ns',
           'find_latency_p99_upper_ns', 'insert_latency_p99_upper_ns')


def read_json(path):
    def reject(value):
        raise ValueError(f'nonfinite JSON constant: {value}')
    return json.loads(path.read_text(), parse_constant=reject)


def stats(values):
    if not values or any(not math.isfinite(v) for v in values):
        raise ValueError('missing or nonfinite observations')
    return {'n': len(values), 'mean': statistics.mean(values),
            'median': statistics.median(values), 'min': min(values),
            'max': max(values)}


def collect(root):
    study_path = root / 'study.json'
    study = read_json(study_path)
    rows, sources, cases = [], [], []
    for case in study['cases']:
        name = case['name']
        if not isinstance(name, str) or Path(name).name != name or name in ('.', '..'):
            raise ValueError('invalid case name')
        path = root / name / 'analysis' / 'summary.json'
        summary = read_json(path)
        config = summary['config']
        if summary['failure_count'] or set(config['variants']) != set(VARIANTS):
            raise ValueError(f'{name}: failed trials or unexpected primary variants')
        expected = config['repetitions']
        trials = summary['trials']
        by_block = {}
        for variant in VARIANTS:
            records = trials[variant]
            if summary['groups'][variant]['n_success'] != expected or len(records) != expected:
                raise ValueError(f'{name}/{variant}: incomplete case')
            by_block[variant] = {t['block']: t['metrics'] for t in records}
            if len(by_block[variant]) != expected:
                raise ValueError(f'{name}/{variant}: duplicate blocks')
        native = by_block['native']
        for variant in VARIANTS:
            current = by_block[variant]
            if set(current) != set(native):
                raise ValueError(f'{name}/{variant}: unpaired blocks')
            base = {'case': name, 'mode': config['mode'], 'variant': variant,
                    'workers': config['workers'], 'layout': config['layout'],
                    'cpus': ','.join(map(str, config['cpus'])),
                    'memory_policy': case.get('memory_policy', config.get('memory_policy')),
                    'memory_nodes': json.dumps(case.get('memory_nodes', config.get('memory_nodes'))),
                    'physical_core_count': case.get('physical_core_count'),
                    'family': case.get('family', ''), 'exploratory': expected < 10}
            for metric in METRICS:
                values = [t[metric] for t in current.values()]
                rows.append({**base, 'metric': metric, **stats(values)})
            if config['mode'] == 'fixed':
                pairs = {'paired_native_elapsed_speedup': [native[b]['elapsed_s'] / current[b]['elapsed_s']
                                                           for b in sorted(native)]}
            else:
                pairs = {f'paired_native_{op}_throughput_ratio':
                         [current[b][f'{op}_ops_s'] / native[b][f'{op}_ops_s'] for b in sorted(native)]
                         for op in ('find', 'insert')}
            for metric, values in pairs.items():
                rows.append({**base, 'metric': metric, **stats(values)})
        sources.append({'case': name, 'path': str(path.relative_to(root)),
                        'sha256': hashlib.sha256(path.read_bytes()).hexdigest()})
        cases.append({'name': name, 'config': config, 'placement': case})
    if not cases:
        raise ValueError('study has no cases')
    return rows, sources, cases, hashlib.sha256(study_path.read_bytes()).hexdigest()


def placement_contrasts(rows):
    """Ratios of case means, NOT paired trials across sequential placements."""
    pairs = [
        (f'physical balanced/compact W={w}',
         f'fixed-physical-compact-w{w}', f'fixed-physical-balanced-w{w}')
        for w in (8, 32)]
    pairs += [
        (f'SMT/physical same workers W={w}', f'fixed-physical-compact-w{w}',
         f'fixed-smt-compact-c{w // 2}-w{w}') for w in (8, 32, 64)]
    pairs += [
        (f'SMT/physical same cores C={c}', f'fixed-physical-compact-w{c}',
         f'fixed-smt-compact-c{c}-w{2*c}') for c in (4, 16, 32, 64)]
    pairs += [
        ('SMT balanced/compact C=16 W=32', 'fixed-smt-compact-c16-w32',
         'fixed-smt-balanced-c16-w32'),
        ('remote/local W=8 bind0', 'fixed-physical-compact-w8',
         'fixed-numa-remote-socket1-w8-bind0'),
        ('interleave/bind0 compact W=8', 'fixed-physical-compact-w8',
         'fixed-numa-compact-w8-interleave'),
        ('interleave/bind0 balanced W=8', 'fixed-physical-balanced-w8',
         'fixed-numa-balanced-w8-interleave')]
    lookup = {(r['case'], r['variant'], r['metric']): r for r in rows}
    contrasts = []
    for label, reference, alternative in pairs:
        for variant in VARIANTS:
            for metric in ('total_ops_s', 'timed_process_cpu_s',
                           'find_latency_mean_ns', 'insert_latency_mean_ns'):
                first = lookup.get((reference, variant, metric))
                second = lookup.get((alternative, variant, metric))
                if first is None or second is None:
                    continue
                if first['mean'] <= 0 or second['mean'] <= 0:
                    raise ValueError('placement comparison requires positive measurements')
                contrasts.append({'comparison': label, 'reference_case': reference,
                                  'alternative_case': alternative, 'variant': variant,
                                  'metric': metric, 'reference': {k: first[k] for k in
                                      ('n', 'mean', 'min', 'max')},
                                  'alternative': {k: second[k] for k in
                                      ('n', 'mean', 'min', 'max')},
                                  'alternative_over_reference': second['mean'] / first['mean']})
    return contrasts


def plots(rows, output, contrasts):
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    import numpy as np

    physical = [r for r in rows if r['mode'] == 'fixed' and r['family'] == 'physical'
                and r['layout'] == 'packed' and r['metric'] == 'total_ops_s']
    if physical:
        fig, axes = plt.subplots(1, 2, figsize=(13, 5))
        for variant in VARIANTS:
            selected = sorted((r for r in physical if r['variant'] == variant),
                              key=lambda r: r['workers'])
            counts = [r['workers'] for r in selected]
            values = [r['mean'] for r in selected]
            errors = [[r['mean'] - r['min'] for r in selected],
                      [r['max'] - r['mean'] for r in selected]]
            axes[0].errorbar(counts, values, yerr=errors, marker='o', capsize=3,
                            label=LABELS[variant])
            if selected[0]['workers'] != 1:
                raise ValueError('physical scaling needs the one-worker baseline')
            efficiency = [value / (values[0] * count) for value, count in zip(values, counts)]
            axes[1].plot(counts, efficiency, marker='o', label=LABELS[variant])
        for ax in axes:
            ax.set_xscale('log', base=2)
            ax.set_xticks(counts, counts)
            ax.set_xlabel('Physical cores / workers (one hardware thread per core)')
            ax.grid(alpha=.25)
            ax.legend()
        axes[0].set_ylabel('Completed operations/s; mean and observed trial range')
        axes[1].set_ylabel('Throughput / (one-worker throughput × workers)')
        axes[1].set_yscale('log')
        fig.suptitle('Compact fixed-work scaling; memory bound to node 0\n'
                     '64 cores spans both sockets; one worker alternates roles; ranges are not CIs')
        fig.tight_layout()
        fig.savefig(output / 'physical_scaling.png', dpi=160)
        plt.close(fig)

    def heatmap(metric, filename, title):
        selected = [r for r in rows if r['metric'] == metric]
        names = list(dict.fromkeys(r['case'] for r in selected))
        if not names:
            return
        lookup = {(r['case'], r['variant']): r for r in selected}
        values = np.array([[lookup[(name, v)]['mean'] for v in VARIANTS] for name in names])
        if np.any(values <= 0):
            raise ValueError('ratios must be positive')
        fig, ax = plt.subplots(figsize=(12, max(4, len(names) * .52 + 2)))
        bound = max(1., float(np.max(np.abs(np.log2(values)))))
        image = ax.imshow(np.log2(values), cmap='RdYlGn', vmin=-bound, vmax=bound, aspect='auto')
        ax.set_xticks(range(len(VARIANTS)), [LABELS[v] for v in VARIANTS])
        ax.set_yticks(range(len(names)), names)
        for i, name in enumerate(names):
            for j, variant in enumerate(VARIANTS):
                row = lookup[(name, variant)]
                ax.text(j, i, f"{row['mean']:.2f}x\n[{row['min']:.2f}, {row['max']:.2f}]",
                        ha='center', va='center', fontsize=8)
        ax.set_title(title + '\nMean paired ratio; brackets = observed trial range, NOT confidence intervals')
        fig.colorbar(image, ax=ax, label='log2 ratio relative to native (higher is better)')
        fig.tight_layout()
        fig.savefig(output / filename, dpi=160)
        plt.close(fig)

    heatmap('paired_native_elapsed_speedup', 'fixed_speedup.png',
            'Fixed-work speedup relative to native (same completed work)')
    heatmap('paired_native_find_throughput_ratio', 'duration_find_ratio.png',
            'Short-duration find throughput relative to native')
    heatmap('paired_native_insert_throughput_ratio', 'duration_insert_ratio.png',
            'Short-duration insert throughput relative to native')

    for mode in ('fixed', 'duration'):
        selected = [r for r in rows if r['mode'] == mode and r['metric'] == 'timed_process_cpu_s']
        names = list(dict.fromkeys(r['case'] for r in selected))
        if not names:
            continue
        lookup = {(r['case'], r['variant']): r for r in selected}
        fig, ax = plt.subplots(figsize=(13, max(4, len(names) * .7 + 2)))
        y = np.arange(len(names))
        width = .15
        for index, variant in enumerate(VARIANTS):
            values = [lookup[(name, variant)]['mean'] for name in names]
            errors = [[v - lookup[(name, variant)]['min'] for name, v in zip(names, values)],
                      [lookup[(name, variant)]['max'] - v for name, v in zip(names, values)]]
            ax.barh(y + (index - 2) * width, values, height=width, xerr=errors,
                    label=LABELS[variant], capsize=2)
        ax.set_yticks(y, names)
        ax.invert_yaxis()
        ax.set_xlabel('Timed process CPU seconds; mean and observed trial range')
        ax.set_title(f'{mode.capitalize()} workload CPU cost (exploratory short study)')
        ax.legend()
        fig.tight_layout()
        fig.savefig(output / f'{mode}_cpu_cost.png', dpi=160)
        plt.close(fig)

    selected = [r for r in contrasts if r['metric'] == 'total_ops_s']
    if selected:
        names = list(dict.fromkeys(r['comparison'] for r in selected))
        lookup = {(r['comparison'], r['variant']): r['alternative_over_reference']
                  for r in selected}
        values = np.array([[lookup[(name, v)] for v in VARIANTS] for name in names])
        bound = max(1., float(np.max(np.abs(np.log2(values)))))
        fig, ax = plt.subplots(figsize=(13, max(5, len(names) * .48 + 2)))
        image = ax.imshow(np.log2(values), cmap='RdYlGn', vmin=-bound, vmax=bound, aspect='auto')
        ax.set_xticks(range(len(VARIANTS)), [LABELS[v] for v in VARIANTS])
        ax.set_yticks(range(len(names)), names)
        for i, name in enumerate(names):
            for j, variant in enumerate(VARIANTS):
                ax.text(j, i, f'{lookup[(name, variant)]:.2f}x', ha='center', va='center')
        ax.set_title('Fixed-work placement contrasts: alternative/reference throughput\n'
                     'Ratios of case means; serial placements, not randomized paired placement trials')
        fig.colorbar(image, ax=ax, label='log2 throughput ratio (higher favors alternative)')
        fig.tight_layout()
        fig.savefig(output / 'placement_contrasts.png', dpi=160)
        plt.close(fig)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input-root', required=True, type=Path)
    parser.add_argument('--output-dir', type=Path)
    args = parser.parse_args()
    root = args.input_root.resolve()
    output = args.output_dir or root / 'overview'
    rows, sources, cases, study_hash = collect(root)
    contrasts = placement_contrasts(rows)
    output.mkdir(parents=True, exist_ok=True)
    with (output / 'scaling.csv').open('w', newline='') as stream:
        writer = csv.DictWriter(stream, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)
    report = {'schema': 1, 'study_sha256': study_hash, 'sources': sources,
              'analysis_script_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              'cases': cases, 'rows': rows, 'placement_contrasts': contrasts, 'notes': [
                  'Exploratory repetitions: observed ranges are NOT confidence intervals.',
                  'Fixed-work speedup is mean paired native/variant elapsed ratio, not ratio of means.',
                  'Duration ratios are operation-specific; do not equate mixed throughput with fixed-work speedup.',
                  'CFL local is the repository implementation, not a certified paper-artifact reproduction.',
                  'One-worker workload alternates operations; other workers have dedicated roles.',
                  'Memory and core-placement configurations must be kept distinct.',
                  'Placement contrasts divide case means, not paired ratios: placements ran sequentially.',
                  'Same-core SMT contrasts also change worker count/contention; same-worker contrasts change core budget.',
                  'Latency quantiles are histogram bucket upper bounds.',
                  'Worst-writer gap is an observed finite maximum, not a starvation guarantee.']}
    (output / 'scaling.json').write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')
    plots(rows, output, contrasts)
    print(f'Wrote {len(cases)} cases and {len(rows)} metric rows to {output}')


if __name__ == '__main__':
    main()
