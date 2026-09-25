#!/usr/bin/env python3
"""Plot the ten completed, fixed-work UpScaleDB cases without rerunning analysis.

Run: python3 integration/upscaledb/overview.py --input-root .worktree/upscaledb
     [--output-dir .worktree/upscaledb/overview]
"""

import argparse
import csv
import hashlib
import json
import math
from pathlib import Path


PRIMARY = ('native', 'refactored', 'bridge_mutex', 'fc', 'fc_pq')
PROFILES = ('profile', 'bridge_mutex_profile', 'fc_profile', 'fc_pq_profile')
LAYOUTS = {'packed': (0, 1, 2, 3), 'split': (0, 1, 32, 33)}
WORKERS = (1, 2, 4, 8, 16)
LABELS = {'native': 'Native', 'refactored': 'Refactored',
          'bridge_mutex': 'Bridge mutex', 'fc': 'FC', 'fc_pq': 'FC-PQ'}
COLORS = {'native': '#34495e', 'refactored': '#228c9b',
          'bridge_mutex': '#d59b2c', 'fc': '#9a65b4', 'fc_pq': '#c84c53'}
ROLES = {1: '1 mixed', 2: '1 find + 1 insert', 4: '2 find + 2 insert',
         8: '4 find + 4 insert', 16: '8 find + 8 insert'}
ELAPSED_ESTIMATOR = 'mean of 10 independent fresh-process trial elapsed_s summaries'
SPEEDUP_ESTIMATOR = 'mean of 10 paired per-trial native/FC-PQ elapsed_s ratios (not ratio of means)'


def require(condition, message):
    if not condition:
        raise ValueError(message)


def number(value, label):
    require(type(value) in (float, int) and math.isfinite(value),
            f'{label}: expected finite number')
    return value


def estimate(stat, label):
    require(type(stat['n']) is int and stat['n'] == 10, f'{label}: expected 10 trials')
    mean = number(stat['mean'], label + '.mean')
    ci = stat['bootstrap95']
    require(type(ci) is list and len(ci) == 2, f'{label}: missing published 95% CI')
    low, high = (number(ci[0], label + '.bootstrap95[0]'),
                 number(ci[1], label + '.bootstrap95[1]'))
    require(0 < mean and 0 < low <= high, f'{label}: expected positive mean and ordered positive CI')
    return mean, low, high


def load_case(root, layout, workers):
    case = root / f'fixed-{layout}-{workers}w'
    source = case / 'analysis' / 'summary.json'
    try:
        raw = source.read_bytes()
        def reject_constant(value):
            raise ValueError(f'non-finite JSON constant: {value}')
        data = json.loads(raw, parse_constant=reject_constant)
        cfg = data['config']
        variants = list(PRIMARY) + (list(PROFILES) if layout == 'packed' and workers == 8 else [])
        expected = {'mode': 'fixed', 'layout': layout, 'cpus': list(LAYOUTS[layout]),
                    'workers': workers, 'finders': 1 if workers == 1 else workers // 2,
                    'inserters': 0 if workers == 1 else workers // 2,
                    'reads': 400000, 'inserts': 400000, 'preload': 100000,
                    'variants': variants, 'repetitions': 10, 'smoke': False,
                    'seconds': 120.0, 'warmup': 5.0, 'seed': 1,
                    'order_seed': 20260923, 'max_inserts': 200000000,
                    'memory_limit_gib': 8, 'timeout_seconds': 900,
                    'wait_proxy_ns': 1000}
        require(data['schema'] == 1, 'unsupported summary schema')
        for field, value in expected.items():
            require(cfg[field] == value, f'config.{field}: expected {value!r}, got {cfg[field]!r}')
        require(data['classification'] == 'candidate_primary_and_separate_profiles',
                'not a completed non-smoke primary case')
        require(data['failure_count'] == 0 and data['raw_failures'] == [],
                'summary reports failed or missing trials')
        flags = data['workload_flags']
        require(flags['environment'] == 'UPS_IN_MEMORY' and flags['db_flags'] == 0 and
                flags['transactions'] is False and flags['recovery'] is False and
                flags['key_type'] == 'UPS_TYPE_UINT64' and flags['record_bytes'] == 8 and
                flags['insert_key_formula'] ==
                '0x8000000000000000|((ordinal*0x5851f42d4c957f2d+'
                '(splitmix64(seed)&0x7fffffffffffffff))&0x7fffffffffffffff)',
                'workload is not the approved in-memory, non-durable permuted-key case')
        groups = data['groups']
        require(set(groups) == set(variants), 'missing or unexpected variant groups')
        elapsed = {}
        for variant in variants:
            group = groups[variant]
            require(group['kind'] == ('profile' if variant in PROFILES else 'primary') and
                    group['n_success'] == 10 and group['n_expected'] == 10 and
                    group['n_failed_or_missing'] == 0 and group['exploratory'] is False,
                    f'{variant}: incomplete or misclassified trial group')
            if variant in PRIMARY:
                stat = group['metrics']['elapsed_s']
                require(stat['exploratory'] is False, f'{variant}: exploratory elapsed estimate')
                elapsed[variant] = estimate(stat, variant + '.elapsed_s')

        matches = [pair for pair in data['paired_comparisons']
                   if pair['category'] == 'overall_primary' and pair['from'] == 'native' and
                   pair['to'] == 'fc_pq' and pair['metric'] == 'elapsed_s']
        require(len(matches) == 1, 'expected exactly one overall primary native/FC-PQ pair')
        pair = matches[0]
        require(pair['ratio_kind'] == 'fixed_work_speedup' and
                pair['instrumentation_gate'] is False and pair['exploratory'] is False and
                pair['n'] == 10 and pair['blocks'] == list(range(10)) and
                pair['used_blocks'] == list(range(10)) and pair['zero_denominator_blocks'] == [],
                'native/FC-PQ comparison is not ten complete paired fixed-work trials')
        speedup = estimate(pair['ratio'], 'native/fc_pq paired ratio')
        return source, hashlib.sha256(raw).hexdigest(), flags, elapsed, speedup
    except (OSError, ValueError, KeyError, TypeError, IndexError) as exc:
        raise ValueError(f'{source}: {exc}') from exc


def plot(rows, output):
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt

    ticks = ['1\nmixed', '2\n1+1', '4\n2+2', '8\n4+4', '16\n8+8']
    caption = ('Identical fixed work: 400k finds + 400k unique inserts, in-memory, no durability. '
               'One worker alternates operations; multiworker cases have equal finder/inserter roles.\n'
               'Both layouts share actual CPU subsets at 1/2 workers; labels do not assert '
               'distinct NUMA placement. Error bars: published 95% bootstrap CIs.')
    for filename, metric in (('fixed_elapsed_scaling.png', 'elapsed_s'),
                             ('fixed_fc_pq_speedup.png', 'fixed_work_speedup')):
        fig, axes = plt.subplots(1, 2, figsize=(15, 6), sharey=True)
        fig.suptitle('UpScaleDB fixed-work scaling' if metric == 'elapsed_s' else
                     'UpScaleDB fixed-work FC-PQ vs native (paired trials)', fontsize=15)
        for ax, (layout, cpus) in zip(axes, LAYOUTS.items()):
            ax.set_title(f'{layout.title()} CPUs {", ".join(map(str, cpus))}')
            ax.set_xticks(range(5), ticks)
            ax.set_xlabel('Workers (find + insert for 2–16)')
            ax.grid(axis='y', alpha=0.25)
            if metric == 'fixed_work_speedup':
                ax.axhline(1, color='#444444', linestyle='--', linewidth=1, label='Parity (1×)')
                series = [('native/fc_pq', 'Native / FC-PQ (paired)', COLORS['fc_pq'])]
            else:
                series = [(variant, LABELS[variant], COLORS[variant]) for variant in PRIMARY]
            for variant, label, color in series:
                selected = [row for row in rows if row['layout'] == layout and
                            row['metric'] == metric and row['variant'] == variant]
                x = range(len(WORKERS))
                ax.plot(x, [row['mean'] for row in selected], marker='o', markersize=4,
                        linewidth=1.6, color=color, label=label)
                # Place the published interval independently of its mean: a percentile
                # bootstrap CI is not mathematically required to contain the mean.
                centers = [(row['ci_low'] + row['ci_high']) / 2 for row in selected]
                ax.errorbar(x, centers,
                            yerr=[[center - row['ci_low'] for center, row in zip(centers, selected)],
                                  [row['ci_high'] - center for center, row in zip(centers, selected)]],
                            fmt='none', ecolor=color, capsize=3, linewidth=1.4)
            ax.legend(fontsize=9, loc='best')
        axes[0].set_ylabel('Elapsed time (s); lower is better' if metric == 'elapsed_s'
                           else 'Mean paired native / FC-PQ elapsed ratio; higher is better')
        detail = ('Mean of paired per-trial elapsed ratios, not ratio of elapsed means.'
                  if metric == 'fixed_work_speedup' else
                  'Mean elapsed per variant across 10 independent fresh-process trials.')
        fig.text(0.5, 0.035, caption + '\n' + detail, ha='center', va='bottom', fontsize=8.5)
        fig.tight_layout(rect=(0, 0.16, 1, 0.94))
        fig.savefig(output / filename, dpi=170)
        plt.close(fig)


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--input-root', type=Path, default=Path('.worktree/upscaledb'))
    parser.add_argument('--output-dir', type=Path, help='default INPUT_ROOT/overview')
    args = parser.parse_args()
    root = args.input_root.resolve()
    output = (args.output_dir or root / 'overview').resolve()
    rows = []
    inputs = []
    common_flags = None
    try:
        for layout in LAYOUTS:
            for workers in WORKERS:
                source, digest, flags, elapsed, speedup = load_case(root, layout, workers)
                if common_flags is None:
                    common_flags = flags
                require(flags == common_flags, f'{source}: workload flags differ across cases')
                case = source.parent.parent
                inputs.append({'case_path': str(case), 'summary_path': str(source),
                               'summary_sha256': digest})
                base = {'layout': layout, 'workers': workers, 'cpus': ','.join(map(str, LAYOUTS[layout])),
                        'roles': ROLES[workers], 'case_path': str(case),
                        'summary_path': str(source), 'summary_sha256': digest, 'n': 10}
                for variant in PRIMARY:
                    mean, low, high = elapsed[variant]
                    rows.append({**base, 'variant': variant, 'metric': 'elapsed_s',
                                 'estimator': ELAPSED_ESTIMATOR,
                                 'ci_estimator': 'published trial-level bootstrap95',
                                 'mean': mean, 'ci_low': low, 'ci_high': high})
                mean, low, high = speedup
                rows.append({**base, 'variant': 'native/fc_pq', 'metric': 'fixed_work_speedup',
                             'estimator': SPEEDUP_ESTIMATOR,
                             'ci_estimator': 'published paired-block bootstrap95',
                             'mean': mean, 'ci_low': low, 'ci_high': high})
        output.mkdir(parents=True, exist_ok=True)
        fields = ('layout', 'workers', 'cpus', 'roles', 'variant', 'metric', 'estimator',
                  'ci_estimator', 'mean', 'ci_low', 'ci_high', 'n', 'case_path',
                  'summary_path', 'summary_sha256')
        with (output / 'overview.csv').open('w', newline='') as stream:
            writer = csv.DictWriter(stream, fieldnames=fields)
            writer.writeheader()
            writer.writerows(rows)
        (output / 'overview.json').write_text(json.dumps({
            'schema': 1, 'description': 'Only ten approved fixed-work cases; profiles and smokes excluded',
            'inputs': inputs, 'plotted_values': rows}, indent=2, allow_nan=False) + '\n')
        plot(rows, output)
    except (OSError, ValueError, ImportError) as exc:
        parser.exit(2, f'overview: {exc}\n')
    print(f'Wrote {output / "fixed_elapsed_scaling.png"}, '
          f'{output / "fixed_fc_pq_speedup.png"}, '
          f'{output / "overview.csv"}, {output / "overview.json"}')


if __name__ == '__main__':
    main()
