"""Analyze only verified raw trials; retain errors, class mix and profile perturbation."""
import html
import json
import math
from pathlib import Path
import statistics

BACKENDS = ('native', 'bridge_mutex', 'fc', 'fc_pq', 'uscl', 'cfl_local', 'mcs')
MIXES = ('reads', 'single', 'batch')
PLACEMENTS = ('shared', 'split')
COLORS = {'native': '#333333', 'bridge_mutex': '#ad6d00', 'fc': '#117747',
          'fc_pq': '#bc2d53', 'uscl': '#5360a6', 'cfl_local': '#88723b', 'mcs': '#247ca6'}


def jain(values):
    total = sum(values)
    return total * total / (len(values) * sum(x * x for x in values)) if total else None

def latency_upper(hist, percent):
    if not hist['count']:
        return None
    rank = math.ceil(hist['count'] * percent)
    cumulative = 0
    for bucket, count in enumerate(hist['log2_bins']):
        cumulative += count
        if cumulative >= rank:
            return 0 if bucket == 0 else (1 << bucket if bucket < 63 else None)
    raise ValueError('latency bins do not sum to recorded samples')


def metric(result):
    workers = result['workers']
    duration = result['duration_ns'] / 1e9
    totals = {key: sum(sum(w['windows'][i][key] for i in range(8)) for w in workers)
              for key in ('reads', 'write_requests', 'records')}
    domains = []
    for indices in ((range(8),) if result['placement'] == 'shared' else (range(4), range(4, 8))):
        service = [workers[id]['service_before_deadline_ns'] for id in indices]
        total = sum(service)
        domains.append({'environment': len(domains), 'workers': list(indices),
                        'requests': sum(sum(workers[id]['windows'][t]['reads'] +
                                            workers[id]['windows'][t]['write_requests']
                                            for t in range(8)) for id in indices),
                        'service_ns': total,
                        'jain_service': jain(service) if result['profile_enabled'] else None,
                        'service_shares': [x / total for x in service] if total and result['profile_enabled'] else None,
                        'window_jain_service': [jain([workers[id]['windows'][t]['service_ns']
                                                       for id in indices])
                                                if result['profile_enabled'] else None
                                                for t in range(8)]})
    return {'reads_per_s': totals['reads'] / duration,
            'write_requests_per_s': totals['write_requests'] / duration,
            'inserted_records_per_s': totals['records'] / duration,
            'total_requests_per_s': (totals['reads'] + totals['write_requests']) / duration,
            'cpu_seconds_per_second': result['process_cpu_ns'] / result['duration_ns'],
            'drain_ns': result['drain_ns'],
            'latency_mean_ns': [w['latency_ns']['sum_ns'] / w['latency_ns']['count']
                                if w['latency_ns']['count'] else None for w in workers],
            'latency_p95_upper_ns': [latency_upper(w['latency_ns'], .95) for w in workers],
            'per_client_cpu_ns': [w['cpu_ns'] for w in workers],
            'max_requester_no_completion_gap_ns': max(w['max_no_completion_gap_ns'] for w in workers),
            'max_requester_submission_gap_ns': max(w['max_submission_gap_ns'] for w in workers),
            'per_client_submission_gap_ns': [w['submission_gap_ns'] for w in workers],
            'per_client_pending_request_ns': [w['pending_request_ns'] for w in workers],
            'zero_progress_client_windows': sum(not (p['reads'] + p['write_requests'])
                                                 for w in workers for p in w['windows']),
            'domains': domains,
            'per_worker_windows': [w['windows'] for w in workers]}


def svg_panel_bars(rows, title, y, unit):
    left, top, width, height = 105, 55, 510, 175
    maximum = max((r['metrics'][y] for r in rows if not r['profile']), default=1) or 1
    lines = [f'<text x="{left}" y="27" font-size="17" font-weight="bold">{html.escape(title)}</text>',
             f'<text x="{left}" y="44" font-size="12">{html.escape(unit)} (mean; min–max across repetitions)</text>',
             f'<line x1="{left}" y1="{top+height}" x2="{left+width}" y2="{top+height}" stroke="#555"/>']
    for fraction in (0, .5, 1):
        level = top + height * (1 - fraction)
        lines.append(f'<line x1="{left}" y1="{level}" x2="{left+width}" y2="{level}" stroke="#ddd"/>')
        lines.append(f'<text x="{left-8}" y="{level+4}" text-anchor="end" font-size="11">{maximum*fraction:,.3g}</text>')
    for idx, backend in enumerate(BACKENDS):
        subset = [r['metrics'][y] for r in rows if r['backend'] == backend and not r['profile']]
        if not subset:
            continue
        center = left + (idx + 0.5) * width / 7
        mean = statistics.mean(subset)
        bar_height = mean / maximum * height
        lines.append(f'<rect x="{center-19:.1f}" y="{top+height-bar_height:.1f}" width="38" height="{bar_height:.1f}" fill="{COLORS[backend]}"/>')
        lo, hi = min(subset) / maximum, max(subset) / maximum
        lines.append(f'<path d="M{center:.1f} {top+height-hi*height:.1f} V{top+height-lo*height:.1f} M{center-5:.1f} {top+height-hi*height:.1f} H{center+5:.1f} M{center-5:.1f} {top+height-lo*height:.1f} H{center+5:.1f}" stroke="black" fill="none"/>')
        lines.append(f'<text x="{center:.1f}" y="{top+height+17}" text-anchor="middle" font-size="11">{html.escape(backend)}</text>')
    return '\n'.join(lines)


def plot_throughput(rows, output):
    # Panels keep request counts and useful records separate; no cross-class unit sum.
    metrics = [('reads_per_s', 'Point reads/s'), ('write_requests_per_s', 'Insert batches or single requests/s'),
               ('inserted_records_per_s', 'Successfully inserted records/s')]
    groups = [(mix, placement) for mix in MIXES for placement in PLACEMENTS]
    parts = ['<svg xmlns="http://www.w3.org/2000/svg" width="1900" height="1670" viewBox="0 0 1900 1670">',
             '<rect width="1900" height="1670" fill="white"/>',
             '<text x="25" y="24" font-size="18">Primary fixed-window throughput; separate service-profile cohort is not mixed here</text>']
    for i, (mix, placement) in enumerate(groups):
        for col, (key, label) in enumerate(metrics):
            subset = [r for r in rows if r['mix'] == mix and r['placement'] == placement]
            parts.append(f'<g transform="translate({col*630},{42+i*270})">')
            parts.append(svg_panel_bars(subset, f'{mix} / {placement}: {label}', key, 'units per second'))
            parts.append('</g>')
    parts.append('</svg>')
    output.write_text('\n'.join(parts) + '\n')


def plot_cpu(rows, output):
    parts = ['<svg xmlns="http://www.w3.org/2000/svg" width="700" height="1670" viewBox="0 0 700 1670">',
             '<rect width="700" height="1670" fill="white"/>',
             '<text x="25" y="24" font-size="17">Primary process CPU cost; worker drain included</text>']
    for i, (mix, placement) in enumerate((m, p) for m in MIXES for p in PLACEMENTS):
        subset = [r for r in rows if r['mix'] == mix and r['placement'] == placement]
        parts.append(f'<g transform="translate(0,{42+i*270})">')
        parts.append(svg_panel_bars(subset, f'{mix} / {placement}: process CPU',
                                    'cpu_seconds_per_second', 'CPU seconds / measured window second'))
        parts.append('</g>')
    parts.append('</svg>')
    output.write_text('\n'.join(parts) + '\n')

def plot_service(rows, output):
    # Bars represent within-environment client share; a split environment
    # has four contenders, not eight competitors across two independent locks.
    parts = ['<svg xmlns="http://www.w3.org/2000/svg" width="1500" height="1370" viewBox="0 0 1500 1370">',
             '<rect width="1500" height="1370" fill="white"/>',
             '<text x="25" y="25" font-size="18">Profile cohort: per-client service share within each serialized environment (mean of repetitions)</text>',
             '<text x="25" y="45" font-size="13">Equal-share reference: 1/8 shared; 1/4 per split environment. Profile changes execution cost.</text>']
    for rowidx, (mix, placement) in enumerate((m, p) for m in MIXES for p in PLACEMENTS):
        y = 85 + rowidx * 210
        parts.append(f'<text x="20" y="{y}" font-size="17">{mix} / {placement}</text>')
        target = 1 / (8 if placement == 'shared' else 4)
        for index, backend in enumerate(BACKENDS):
            x = 65 + index * 200
            matches = [r for r in rows if r['mix'] == mix and r['placement'] == placement
                       and r['backend'] == backend and r['profile']]
            parts.append(f'<text x="{x}" y="{y+22}" font-size="12">{backend}</text>')
            if not matches:
                parts.append(f'<text x="{x}" y="{y+78}" font-size="12">missing</text>')
                continue
            for domain in range(1 if placement == 'shared' else 2):
                available = [r['metrics']['domains'][domain]['service_shares'] for r in matches]
                available = [v for v in available if v is not None]
                if not available:
                    continue
                averaged = [statistics.mean(v[i] for v in available) for i in range(len(available[0]))]
                base = y + 40 + domain * 79
                parts.append(f'<text x="{x}" y="{base+12}" font-size="11">env{domain}; scale 0–100%</text>')
                parts.append(f'<line x1="{x}" y1="{base+62}" x2="{x+165}" y2="{base+62}" stroke="#aaa"/>')
                parts.append(f'<line x1="{x}" y1="{base+62-target*60:.1f}" x2="{x+165}" y2="{base+62-target*60:.1f}" stroke="#777" stroke-dasharray="3,3"/>')
                for client, value in enumerate(averaged):
                    height = value * 60
                    parts.append(f'<rect x="{x+25+client*15}" y="{base+62-height:.1f}" width="10" height="{height:.1f}" fill="{COLORS[backend]}"/>')
        parts.append(f'<line x1="20" y1="{y+190}" x2="1470" y2="{y+190}" stroke="#ddd"/>')
    parts.append('</svg>')
    output.write_text('\n'.join(parts) + '\n')


def analyze(raw_root):
    raw_root = raw_root.resolve(strict=True)
    manifest = json.loads((raw_root / 'manifest.json').read_text())
    if manifest['schema'] != 1 or manifest['backends'] != list(BACKENDS):
        raise ValueError('not a heterogeneous-clients root')
    output = raw_root / 'analysis'
    output.mkdir(exist_ok=False)
    rows, failures = [], []
    expected_names = {
        f'{mix}-{placement}-r{rep}-{variant}'
        for rep in range(1, manifest['repetitions'] + 1)
        for mix in MIXES for placement in PLACEMENTS
        for variant in (*BACKENDS, 'profile', *(f'{b}_profile' for b in BACKENDS[1:]))
    }
    seen_names = set()
    for directory in sorted(raw_root.iterdir()):
        if not directory.is_dir() or directory == output:
            continue
        seen_names.add(directory.name)
        if not (directory / 'status.json').is_file():
            failures.append({'trial': directory.name, 'status': None,
                             'reason': 'interrupted before status was written',
                             'stdout': str(directory / 'stdout.txt'),
                             'stderr': str(directory / 'stderr.txt')})
            continue
        status = json.loads((directory / 'status.json').read_text())
        if not status['ok']:
            failures.append({'trial': directory.name, 'status': status,
                             'stdout': str(directory / 'stdout.txt'),
                             'stderr': str(directory / 'stderr.txt')})
            continue
        result = json.loads((directory / 'result.json').read_text())
        if result['status'] != 'ok' or any(o['integrity_status'] or o['count_status'] or
                o['cursor_status'] or o['bad'] or o['seen'] != o['expected'] or
                o['count'] != o['expected'] for o in result['oracle']):
            raise ValueError(f'{directory.name}: oracle is not valid')
        rows.append({'trial': directory.name, 'raw': str(directory / 'result.json'),
                     'mix': status['mix'], 'placement': status['placement'],
                     'rep': status['rep'], 'seed': status['seed'],
                     'backend': status['variant'].replace('_profile', '')
                                if status['variant'] != 'profile' else 'native',
                     'profile': status['profile'], 'metrics': metric(result)})
    missing_trials = sorted(expected_names - seen_names)
    paired = []
    by_key = {(r['mix'], r['placement'], r['rep'], r['backend'], r['profile']): r for r in rows}
    for mix in MIXES:
        for placement in PLACEMENTS:
            for rep in range(1, manifest['repetitions'] + 1):
                fc = by_key.get((mix, placement, rep, 'fc', False))
                pq = by_key.get((mix, placement, rep, 'fc_pq', False))
                fc_service = by_key.get((mix, placement, rep, 'fc', True))
                pq_service = by_key.get((mix, placement, rep, 'fc_pq', True))
                if fc and pq:
                    paired.append({'mix': mix, 'placement': placement, 'rep': rep,
                        'fc_pq_vs_fc_total_requests_ratio':
                            pq['metrics']['total_requests_per_s'] / fc['metrics']['total_requests_per_s']
                            if fc['metrics']['total_requests_per_s'] else None,
                        'fc_pq_vs_fc_reads_ratio':
                            pq['metrics']['reads_per_s'] / fc['metrics']['reads_per_s']
                            if fc['metrics']['reads_per_s'] else None,
                        'fc_pq_vs_fc_inserted_records_ratio':
                            pq['metrics']['inserted_records_per_s'] / fc['metrics']['inserted_records_per_s']
                            if fc['metrics']['inserted_records_per_s'] else None,
                        'fc_pq_vs_fc_cpu_ratio':
                            pq['metrics']['cpu_seconds_per_second'] / fc['metrics']['cpu_seconds_per_second']
                            if fc['metrics']['cpu_seconds_per_second'] else None,
                        'profile_fc_service_jain_by_environment':
                            [d['jain_service'] for d in fc_service['metrics']['domains']]
                            if fc_service else None,
                        'profile_fc_pq_service_jain_by_environment':
                            [d['jain_service'] for d in pq_service['metrics']['domains']]
                            if pq_service else None})
    perturbation = []
    for row in rows:
        if row['profile']:
            primary = by_key.get((row['mix'], row['placement'], row['rep'], row['backend'], False))
            if primary:
                perturbation.append({'mix': row['mix'], 'placement': row['placement'],
                    'rep': row['rep'], 'backend': row['backend'],
                    'profile_vs_primary_requests_ratio':
                        row['metrics']['total_requests_per_s'] / primary['metrics']['total_requests_per_s']
                        if primary['metrics']['total_requests_per_s'] else None,
                    'profile_vs_primary_records_ratio':
                        row['metrics']['inserted_records_per_s'] / primary['metrics']['inserted_records_per_s']
                        if primary['metrics']['inserted_records_per_s'] else None,
                    'profile_vs_primary_reads_ratio':
                        row['metrics']['reads_per_s'] / primary['metrics']['reads_per_s']
                        if primary['metrics']['reads_per_s'] else None,
                    'profile_vs_primary_write_requests_ratio':
                        row['metrics']['write_requests_per_s'] / primary['metrics']['write_requests_per_s']
                        if primary['metrics']['write_requests_per_s'] else None,
                    'profile_vs_primary_cpu_ratio':
                        row['metrics']['cpu_seconds_per_second'] / primary['metrics']['cpu_seconds_per_second']
                        if primary['metrics']['cpu_seconds_per_second'] else None})
    summary = {'schema': 1, 'raw_manifest': str(raw_root / 'manifest.json'),
               'expected_trials': manifest['trials'], 'successful_trials': len(rows),
               'missing_trials': missing_trials, 'failed_trials': failures,
               'rows': rows, 'fc_pq_vs_fc_paired': paired,
               'profile_perturbation_paired': perturbation,
               'interpretation': {'profile_service_ns': 'elapsed serialized callback/Env-mutex body time; not CPU time',
                   'latency_and_cpu': 'latency histogram and worker/process CPU include completed drain requests; throughput, service shares and progress bins exclude drain',
                   'split_fairness': 'compare only four clients within the same environment; global client JFI is not a fairness test',
                   'pending_gap': 'submission/return gaps and zero-progress windows are observed; continuous selectability and starvation bounds are not established',
                   'scheduler_cost': 'mix-shift can change total throughput; no scheduler-only cost inferred from end-to-end effects',
                   'inference': 'three repetitions are exploratory ranges, not statistical significance'}}
    (output / 'summary.json').write_text(json.dumps(summary, indent=2, sort_keys=True) + '\n')
    plot_throughput(rows, output / 'throughput.svg')
    plot_service(rows, output / 'service-share.svg')
    plot_cpu(rows, output / 'cpu.svg')
    lines = [f"Observed {len(rows)}/{manifest['trials']} verified trials; {len(failures)} failed; {len(missing_trials)} missing.",
             'Two-second fixed windows; eight clients on CPUs0-7; total initial data constant.',
             'Primary throughput and separately instrumented per-environment service-share figures:']
    for row in paired:
        value = row['fc_pq_vs_fc_total_requests_ratio']
        lines.append(f"{row['mix']}/{row['placement']} rep{row['rep']}: FC-PQ/FC total requests ratio {value:.3f}" if value else
                     f"{row['mix']}/{row['placement']} rep{row['rep']}: ratio unavailable")
    lines += ['Counts and useful inserted records/s are separate. FC-PQ/FC request mix may shift.',
              'Profile perturbation, per-client windows and oracle/error evidence are in summary.json and raw trials.',
              'Split fairness domains have four clients each; requester pending gaps prohibit a starvation theorem.']
    (output / 'summary.txt').write_text('\n'.join(lines) + '\n')
    print(f"{len(rows)}/{manifest['trials']} valid; {len(failures)} failures; {output}")
    return int(bool(failures or missing_trials) or len(rows) != manifest['trials'])
