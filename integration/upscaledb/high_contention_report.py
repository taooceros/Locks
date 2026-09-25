#!/usr/bin/env python3
"""Analyze immutable high-contention UpScaleDB trial records and render a Chinese report.

This plotting/analysis source is not measurement source. An existing analysis
folder is never overwritten; a revised presentation belongs in a fresh folder.
"""
import base64
import csv
import html
import itertools
import json
import math
from pathlib import Path
import statistics

import boundary_tables as base
import high_contention as study


def scan(root, manifest, directory, schedule):
    status_path = root / directory / 'status.json'
    status = base.load(status_path) if status_path.exists() else None
    hashes = {row['id']: row['record_sha256'] for row in status['records']} if status else {}
    if status:
        base.require(status['expected'] == len(schedule) and status['attempted'] <= len(schedule) and
                     status['successful'] + status['failed'] == status['attempted'] and
                     status['missing'] == len(schedule) - status['attempted'] and
                     len(hashes) == len(status['records']), 'status count mismatch')
    run_manifest_path = root / directory / 'manifest.json'
    manifest_missing = not run_manifest_path.exists() and (root / directory).exists()
    if run_manifest_path.exists():
        run_manifest = base.load(run_manifest_path)
        base.require(run_manifest['prepared_sha256'] == base.digest(root / 'prepared.json') and
                     run_manifest['manifest_sha256'] == base.digest(root / 'manifest.json') and
                     run_manifest['schedule'] == schedule, 'run manifest provenance mismatch')
    trials, results = [], []
    for item in schedule:
        record_dir = root / directory / item['id']
        record_path = record_dir / 'record.json'
        trial = {'id': item['id'], 'rep': item['rep'], 'backend': item['backend'],
                 'workers': item['workers'], 'routing': item['routing'], 'seed': item['seed'],
                 'status': 'missing', 'record_path': str(record_path)}
        if record_path.exists():
            try:
                base.require(not manifest_missing, 'run manifest absent for recorded process')
                sha = base.digest(record_path)
                trial['record_sha256'] = sha
                if status:
                    base.require(hashes.get(item['id']) == sha, 'missing/mismatched status record hash')
                record = base.load(record_path)
                base.require(record['command'] == item['command'], 'process command mismatch')
                for name in ('stdout', 'stderr', 'invocation.json'):
                    base.require(base.digest(record_dir / name) == record[name + '_sha256'],
                                 name + ' hash mismatch')
                base.require(record['success'], record.get('error', 'recorded process failure'))
                result = base.load(record_dir / 'stdout')
                base.require(result == record['result'], 'raw output differs from record')
                study.validate(result, item)
                trial['status'] = 'ok'
                results.append((item, result))
            except (OSError, ValueError, KeyError, TypeError) as exc:
                trial['status'] = 'failed'
                trial['error'] = type(exc).__name__ + ': ' + str(exc)
        elif record_dir.exists():
            trial['status'] = 'failed'
            trial['error'] = 'partial invocation: inspect retained raw files'
        trials.append(trial)
    return trials, results


def csv_file(path, rows, columns):
    with path.open('x', newline='') as target:
        writer = csv.DictWriter(target, fieldnames=columns)
        writer.writeheader()
        writer.writerows({key: row.get(key, '') for key in columns} for row in rows)


def describe(points):
    return {'points': points, 'available_n': len(points),
            'median': statistics.median(points) if points else None,
            'observed_min': min(points) if points else None,
            'observed_max': max(points) if points else None}


def screen(points):
    if len(points) != study.REPETITIONS:
        return 'incomplete'
    if all(point > 1.05 for point in points):
        return 'consistent >5% increase'
    if all(point < .95 for point in points):
        return 'consistent >5% decrease'
    return 'mixed/small (not equivalence)'

def analyze(root, output_dir=None):
    manifest_path = root / 'manifest.json'
    manifest = base.load(manifest_path)
    study.validate_manifest(manifest)
    prepared = base.load(root / 'prepared.json')
    base.require(prepared['schema'] == study.SCHEMA and prepared['status'] == 'prepared' and
                 manifest['output_root'] == str(root) and
                 prepared['files'][str(manifest_path)] == base.digest(manifest_path),
                 'prepared provenance mismatch')
    # Analysis runs on the recorded raw artifacts; the current plotting source may
    # change later, but frozen measured binaries and sources must remain untouched.
    base.assert_frozen(prepared['files'])
    out = output_dir if output_dir is not None else root / 'analysis'
    out.mkdir(parents=True, exist_ok=False)
    trials, primary = scan(root, manifest, 'run', manifest['schedule'])
    diagnostic_trials, diagnostics = scan(root, manifest, 'diagnostics', manifest['diagnostic_schedule'])
    measurements, per_table, per_worker = [], [], []
    primary_lookup = {}
    for item, result in primary:
        key = (item['workers'], item['routing'], item['backend'], item['rep'])
        role_rows, table_rows = base.measurements(item, result)
        for row in role_rows:
            row.update(workers=item['workers'], routing=item['routing'])
            measurements.append(row)
        for row in table_rows:
            row.update(workers=item['workers'], routing=item['routing'])
            per_table.append(row)
        total_on_time = sum(row['on_time'] for row in role_rows)
        completed = sum(row['completed'] for row in role_rows)
        hot_on_time = sum(row['on_time'] for row in table_rows if row['table'] == 0)
        totals = {'trial': item['id'], 'rep': item['rep'], 'workers': item['workers'],
                  'routing': item['routing'], 'backend': item['backend'], 'role': 'total',
                  'throughput_ops_s': total_on_time / manifest['seconds'], 'completed': completed,
                  'on_time': total_on_time, 'hot_table_fraction': hot_on_time / total_on_time if total_on_time else None,
                  'late': completed - total_on_time,
                  'request_mean_ns': None, 'request_p99_upper_ns': None,
                  'process_cpu_ns_per_completed_op': result['process_cpu_ns'] / completed if completed else None,
                  'process_cpu_ns': result['process_cpu_ns'], 'drain_ns': result['drain_ns']}
        measurements.append(totals)
        primary_lookup[key] = totals
        for w in result['workers']:
            per_worker.append({'trial': item['id'], 'rep': item['rep'], 'workers': item['workers'],
                               'routing': item['routing'], 'backend': item['backend'], 'worker': w['id'],
                               'finds': sum(t['finds'] for t in w['tables']),
                               'inserts': sum(t['inserts'] for t in w['tables']),
                               'hot_table_operations': w['tables'][0]['finds'] + w['tables'][0]['inserts'],
                               'worker_cpu_ns': w['cpu_ns'], 'end_ns': w['end_ns']})
    by_metric = {(r['workers'], r['routing'], r['backend'], r['rep'], r['role']): r for r in measurements}
    metrics = ('throughput_ops_s', 'request_mean_ns', 'request_p99_upper_ns',
               'process_cpu_ns_per_completed_op')
    contrasts = []

    def compare(kind, left, right, role, metric, labels):
        points, paired = [], []
        for rep in range(1, study.REPETITIONS + 1):
            a = by_metric.get((*left, rep, role))
            b = by_metric.get((*right, rep, role))
            if a and b and a.get(metric) is not None and b.get(metric) is not None and b[metric] > 0:
                points.append(a[metric] / b[metric]); paired.append(rep)
        contrasts.append({'contrast': kind, 'numerator': labels[0], 'reference': labels[1],
                          'role': role, 'metric': metric, 'paired_repetitions': paired,
                          'ratio': describe(points), 'screen': screen(points),
                          'orientation': 'higher is better' if metric == 'throughput_ops_s' else 'lower is better'})

    for backend, workers in itertools.product(manifest['backends'], study.WORKERS):
        for hot, cold in (('hot90', 'uniform'), ('hot100', 'hot90'), ('hot100', 'uniform')):
            for role in ('total', 'finds', 'inserts'):
                for metric in metrics:
                    if role == 'total' and metric in ('request_mean_ns', 'request_p99_upper_ns'):
                        continue
                    compare('routing at fixed workers', (workers, hot, backend),
                            (workers, cold, backend), role, metric,
                            (f'{backend}/{workers}/{hot}', f'{backend}/{workers}/{cold}'))
    for backend, routing in itertools.product(manifest['backends'], study.ROUTING):
        for larger, smaller in ((16, 8), (32, 16), (32, 8)):
            for role in ('total', 'finds', 'inserts'):
                for metric in metrics:
                    if role == 'total' and metric in ('request_mean_ns', 'request_p99_upper_ns'):
                        continue
                    compare('workers at fixed routing', (larger, routing, backend),
                            (smaller, routing, backend), role, metric,
                            (f'{backend}/{larger}/{routing}', f'{backend}/{smaller}/{routing}'))
    for workers, routing, other in itertools.product(study.WORKERS, study.ROUTING,
                                                       (b for b in manifest['backends'] if b != 'fc_pq')):
        for role in ('total', 'finds', 'inserts'):
            for metric in metrics:
                if role == 'total' and metric in ('request_mean_ns', 'request_p99_upper_ns'):
                    continue
                compare('FC-PQ/backend at matched condition', (workers, routing, 'fc_pq'),
                        (workers, routing, other), role, metric,
                        (f'fc_pq/{workers}/{routing}', f'{other}/{workers}/{routing}'))
    overlap_rows = []
    for item, result in diagnostics:
        counts = [[0] * item['workers'] for _ in range(32)]
        for worker in result['workers']:
            for table, hist in enumerate(worker['api_overlap_counts']):
                counts[table] = [a + b for a, b in zip(counts[table], hist)]
        for table, hist in enumerate(counts):
            submissions = sum(hist)
            overlap_rows.append({'trial': item['id'], 'rep': item['rep'], 'backend': item['backend'],
                'routing': item['routing'], 'table': table, 'submissions': submissions,
                'overlapping_submissions': sum(hist[1:]),
                'overlap_fraction': sum(hist[1:]) / submissions if submissions else None,
                'outstanding_at_submission_counts_1_to_32': hist})
    perturbations = []
    for item, result in diagnostics:
        primary_row = primary_lookup.get((32, item['routing'], item['backend'], item['rep']))
        on_time = sum(t[role + '_on_time'] for w in result['workers'] for t in w['tables']
                      for role in ('finds', 'inserts'))
        diag_rate = on_time / manifest['seconds']
        primary_rate = primary_row['throughput_ops_s'] if primary_row else None
        selected = [r for r in overlap_rows if r['trial'] == item['id']]
        observations = sum(r['submissions'] for r in selected)
        overlap = sum(r['overlapping_submissions'] for r in selected)
        maximum = max((index + 1 for row in selected for index, count in enumerate(
            row['outstanding_at_submission_counts_1_to_32']) if count), default=0)
        perturbations.append({'trial': item['id'], 'rep': item['rep'], 'backend': item['backend'],
            'routing': item['routing'], 'diagnostic_ops_s': diag_rate,
            'paired_primary_ops_s': primary_rate,
            'diagnostic_over_primary_rate': diag_rate / primary_rate if primary_rate else None,
            'submissions': observations, 'overlapping_submissions': overlap,
            'overlap_fraction': overlap / observations if observations else None,
            'maximum_outstanding_at_submission': maximum,
            'hot_table_submissions': selected[0]['submissions']})
    summary = {'schema': study.SCHEMA, 'manifest_sha256': base.digest(manifest_path),
        'prepared_sha256': base.digest(root / 'prepared.json'),
        'primary': {'expected': len(trials), 'ok': sum(t['status'] == 'ok' for t in trials),
                    'failed': sum(t['status'] == 'failed' for t in trials),
                    'missing': sum(t['status'] == 'missing' for t in trials), 'trials': trials},
        'diagnostics': {'expected': len(diagnostic_trials),
                        'ok': sum(t['status'] == 'ok' for t in diagnostic_trials),
                        'failed': sum(t['status'] == 'failed' for t in diagnostic_trials),
                        'missing': sum(t['status'] == 'missing' for t in diagnostic_trials),
                        'trials': diagnostic_trials, 'perturbations': perturbations,
                        'api_overlap_per_table': overlap_rows},
        'measurements': measurements, 'per_table_progress': per_table,
        'per_worker_progress': per_worker, 'contrasts': contrasts,
        'caveats': manifest['caveats'], 'hypotheses': manifest['hypotheses'],
        'measurement_source_sha256': {name: prepared['files'][str(root / 'sources' / name)]
                                      for name in ('boundary_tables.py', 'boundary_tables.cc', 'high_contention.py')},
        'plotting_source_sha256': base.digest(Path(__file__)),
        'paired_seeds': manifest['workload_seeds'], 'excluded_backends': manifest['excluded_backends']}
    base.save(out / 'summary.json', summary)
    for name, selected in (('primary', trials), ('diagnostics', diagnostic_trials)):
        csv_file(out / (name + '-trials.csv'),
                 [{**t, 'error': t.get('error', '')} for t in selected],
                 ('id', 'rep', 'backend', 'workers', 'routing', 'seed', 'status',
                  'record_path', 'record_sha256', 'error'))
    csv_file(out / 'measurements.csv', measurements,
             ('trial', 'rep', 'workers', 'routing', 'backend', 'role', 'completed', 'on_time',
              'late', 'throughput_ops_s', 'hot_table_fraction', 'request_samples', 'request_mean_ns',
              'request_p50_upper_ns', 'request_p95_upper_ns', 'request_p99_upper_ns',
              'request_p999_upper_ns', 'request_max_ns', 'process_cpu_ns_per_completed_op',
              'process_cpu_ns', 'worker_cpu_ns', 'drain_ns', 'peak_rss_kib', 'final_records'))
    csv_file(out / 'per-table.csv', per_table,
             ('trial', 'rep', 'workers', 'routing', 'backend', 'role', 'table',
              'completed', 'on_time', 'throughput_ops_s'))
    csv_file(out / 'per-worker.csv', per_worker,
             ('trial', 'rep', 'workers', 'routing', 'backend', 'worker', 'finds',
              'inserts', 'hot_table_operations', 'worker_cpu_ns', 'end_ns'))
    csv_file(out / 'diagnostic-perturbations.csv', perturbations,
             ('trial', 'rep', 'backend', 'routing', 'diagnostic_ops_s', 'paired_primary_ops_s',
              'diagnostic_over_primary_rate', 'submissions', 'overlapping_submissions',
              'overlap_fraction', 'maximum_outstanding_at_submission', 'hot_table_submissions'))
    csv_file(out / 'api-overlap-tables.csv',
             [{**r, 'outstanding_at_submission_counts_1_to_32': json.dumps(
                 r['outstanding_at_submission_counts_1_to_32'])} for r in overlap_rows],
             ('trial', 'rep', 'backend', 'routing', 'table', 'submissions',
              'overlapping_submissions', 'overlap_fraction',
              'outstanding_at_submission_counts_1_to_32'))
    csv_file(out / 'contrasts.csv',
             [{**{k: v for k, v in c.items() if k != 'ratio'},
               **{**c['ratio'], 'points': json.dumps(c['ratio']['points'])},
               'paired_repetitions': json.dumps(c['paired_repetitions'])} for c in contrasts],
             ('contrast', 'numerator', 'reference', 'role', 'metric', 'paired_repetitions',
              'available_n', 'median', 'observed_min', 'observed_max', 'points', 'screen', 'orientation'))
    figures = plot(out, measurements, perturbations, summary)
    render(out, summary, manifest, figures)
    return out / 'summary.json'


def plot(out, rows, perturbations, summary):
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    from matplotlib.ticker import FuncFormatter, LogLocator, SymmetricalLogLocator
    styles = {name: (base.BACKEND_COLORS[index], ('o', 's', '^', 'D', 'v', 'P', 'X', '<', '>', '*')[index])
              for index, name in enumerate(base.BACKENDS)}
    graphics = []
    for role, metric, title, filename, unit in (
        ('total', 'throughput_ops_s', 'Total on-time throughput', 'throughput', 'operations/s'),
        ('total', 'process_cpu_ns_per_completed_op', 'Process CPU per completed operation', 'cpu-per-op', 'ns/completion'),
        ('finds', 'request_mean_ns', 'Find requester mean latency', 'find-mean-latency', 'ns'),
        ('inserts', 'request_mean_ns', 'Insert requester mean latency', 'insert-mean-latency', 'ns'),
        ('finds', 'request_p99_upper_ns', 'Find requester p99 bin upper bound', 'find-p99', 'ns'),
        ('inserts', 'request_p99_upper_ns', 'Insert requester p99 bin upper bound', 'insert-p99', 'ns')):
        values = [r[metric] for r in rows if r['role'] == role and r.get(metric) is not None]
        maximum = max(values, default=0)
        minimum = min(values, default=0)
        fig, axes = plt.subplots(1, 3, figsize=(17, 5), sharey=True)
        for ax, routing in zip(axes, study.ROUTING):
            for backend in base.BACKENDS:
                color, marker = styles[backend]
                medians = []
                for workers in study.WORKERS:
                    points = [r[metric] for r in rows if (r['role'], r['routing'], r['backend'], r['workers']) ==
                              (role, routing, backend, workers) and r.get(metric) is not None]
                    medians.append(statistics.median(points) if points else math.nan)
                    ax.scatter([workers] * len(points), points, s=11, alpha=.35, color=color)
                ax.plot(study.WORKERS, medians, marker=marker, markersize=4,
                        linewidth=1, color=color, label=base.BACKEND_LABELS[backend])
            ax.set(title=routing, xticks=study.WORKERS, xlabel='Physical workers', ylabel=unit)
            ax.set_xlim(6, 34)
            if minimum > 0:
                ax.set_yscale('log')
                ax.set_ylim(minimum / 1.25, maximum * 1.25)
                ax.yaxis.set_major_locator(LogLocator(base=10, subs=(1, 2, 5)))
            else:
                threshold = min((v for v in values if v > 0), default=1)
                ax.set_yscale('symlog', linthresh=threshold)
                ax.set_ylim(0, max(1, maximum * 1.25))
                ax.yaxis.set_major_locator(SymmetricalLogLocator(base=10, linthresh=threshold))
            ax.yaxis.set_major_formatter(FuncFormatter(lambda value, _: f'{value:g}'))
            ax.grid(alpha=.25, which='both')
        fig.legend(*axes[0].get_legend_handles_labels(), loc='lower center', ncol=5,
                   bbox_to_anchor=(.5, -.02), frameon=False)
        fig.suptitle(title + ': individual repetitions and medians; missing quantiles are not zero')
        fig.tight_layout(rect=(0, .13, 1, .92))
        for suffix in ('png', 'svg'):
            fig.savefig(out / (filename + '.' + suffix), dpi=150)
        plt.close(fig)
        graphics.append(filename)
    fig, ax = plt.subplots(figsize=(9, 5))
    for backend in study.DIAGNOSTIC_BACKENDS:
        color, marker = styles[backend]
        for routing in study.ROUTING:
            values = [r['overlap_fraction'] for r in perturbations if r['backend'] == backend and
                      r['routing'] == routing and r['overlap_fraction'] is not None]
            x = study.ROUTING.index(routing)
            ax.scatter([x] * len(values), values, color=color, alpha=.4, s=20)
        medians = [statistics.median(values) if (values := [r['overlap_fraction'] for r in perturbations
                   if r['backend'] == backend and r['routing'] == routing and
                   r['overlap_fraction'] is not None]) else math.nan for routing in study.ROUTING]
        ax.plot(range(3), medians, marker=marker, color=color, label=base.BACKEND_LABELS[backend])
    ax.set(xticks=range(3), xticklabels=study.ROUTING, ylim=(0, 1),
           ylabel='Fraction of submissions with concurrent same-table API calls',
           title='Separate diagnostic build: API overlap proxy, not lock wait-queue depth')
    ax.grid(alpha=.25)
    ax.legend()
    fig.tight_layout()
    for suffix in ('png', 'svg'):
        fig.savefig(out / ('api-overlap.' + suffix), dpi=150)
    plt.close(fig)
    graphics.append('api-overlap')
    base.save(out / 'figures.json', {'figures': graphics,
              'matplotlib_version': matplotlib.__version__,
              'plotting_source_sha256': base.digest(Path(__file__)),
              'measurement_source_sha256': summary['measurement_source_sha256']})
    return graphics


def render(out, summary, manifest, figures):
    lines = []
    def paragraph(text):
        lines.append('<p>' + html.escape(text) + '</p>')
    def section(title):
        lines.append('<h2>' + html.escape(title) + '</h2>')
    def table(headers, data):
        lines.append('<div class="scroll"><table><tr>' + ''.join('<th>' + html.escape(str(h)) + '</th>' for h in headers) + '</tr>')
        for row in data:
            lines.append('<tr>' + ''.join('<td>' + html.escape(str(v)) + '</td>' for v in row) + '</tr>')
        lines.append('</table></div>')
    def fmt(value, scale=1):
        return 'NA' if value is None else f'{value / scale:,.3f}'
    section('UpScaleDB高争用：32个独立环境、十种锁')
    p, d = summary['primary'], summary['diagnostics']
    paragraph(f'预定主实验{p["expected"]}次：通过{p["ok"]}、失败{p["failed"]}、缺失{p["missing"]}；独立API重叠诊断{d["expected"]}次：通过{d["ok"]}、失败{d["failed"]}、缺失{d["missing"]}。所有失败与缺失在trials.csv、summary.json和原始目录中保留，未选择性替换。')
    paragraph('32个预建独立environment/句柄，总初始记录32768，8字节键值；每个固定物理核工作线程交替find/唯一insert，所有句柄经同一批活线程只读warmup后由共同gate放行。8/16/32线程×均匀/90%热表目标/100%热表×十后端，5个配对种子；每次2秒。热表目标不是观测到的精确比例，且改变B树增长及局部性。')
    paragraph('方向筛选仅在五个同序号完整配对比值都>1.05或都<0.95时称一致差异；其余标为混合/小幅，不代表等价、显著性或公平性优势。吞吐以窗口内完成数计，CPU涵盖放行至join及drain，延迟直方图分位数是桶上界。')
    section('每种条件：总吞吐、CPU与请求延迟')
    rows = summary['measurements']
    cells = []
    for workers, routing, backend in itertools.product(study.WORKERS, study.ROUTING, manifest['backends']):
        selected = [r for r in rows if (r['workers'], r['routing'], r['backend']) == (workers, routing, backend)]
        total = [r for r in selected if r['role'] == 'total']
        finds = [r for r in selected if r['role'] == 'finds']
        inserts = [r for r in selected if r['role'] == 'inserts']
        def values(group, key):
            numbers = [r[key] for r in group if r.get(key) is not None]
            return fmt(statistics.median(numbers)) if numbers else 'NA'
        cells.append((workers, routing, backend, f'{len(total)}/5',
                      ', '.join(f'{r["rep"]}:{fmt(r["throughput_ops_s"])}' for r in total) or 'NA',
                      values(total, 'throughput_ops_s'), values(total, 'hot_table_fraction'),
                      values(total, 'process_cpu_ns_per_completed_op'),
                      values(finds, 'request_mean_ns'), values(inserts, 'request_mean_ns'),
                      values(finds, 'request_p99_upper_ns'),
                      sum(t['status'] == 'failed' for t in p['trials'] if t['workers'] == workers and
                          t['routing'] == routing and t['backend'] == backend),
                      sum(t['status'] == 'missing' for t in p['trials'] if t['workers'] == workers and
                          t['routing'] == routing and t['backend'] == backend)))
    table(('worker', '路由', '后端', '通过', '逐次总ops/s(rep:value)', '中位ops/s', '观测表0占比',
           'CPU ns/完成操作', 'find均值ns', 'insert均值ns', 'find p99桶上界ns', '失败', '缺失'), cells)
    section('预声明对照：优势、相持与劣势')
    selected_contrasts = [c for c in summary['contrasts'] if c['role'] == 'total' and
                          c['metric'] == 'throughput_ops_s']
    table(('对照', '分子', '参照', '配对重复', '逐次吞吐比', '结论'),
          ((c['contrast'], c['numerator'], c['reference'], c['paired_repetitions'],
            [round(v, 4) for v in c['ratio']['points']], c['screen']) for c in selected_contrasts))
    paragraph('FC-PQ对FC、MCS、CLH、USCL、CFL-local以及所有其他后端的配对对照均完整保留；全部角色和延迟/CPU对照见contrasts.csv。FC-PQ不需要超过FC才能有公平性价值；本矩阵没有异质成本与服务份额，不能证明这种价值。')
    section('独立API重叠诊断（非锁排队深度）')
    paragraph('仅FC、FC-PQ、MCS的32线程条件，各三次；共享原配对种子，但另编译含每环境atomic计数的诊断二进制，主计时路径无此计数。提交时看到的其他未返回API包括执行、发布、等待、返回阶段，不能等同锁持有者、队列长度或准确等待时间。诊断与同期主实验吞吐比只用于揭示扰动，不校正主结果。')
    table(('重复', '路由', '后端', '提交总数', '观察重叠比例', '最高观测并发API数',
           '表0请求数', '诊断/配对主吞吐比'),
          ((r['rep'], r['routing'], r['backend'], r['submissions'], fmt(r['overlap_fraction']),
            r['maximum_outstanding_at_submission'], r['hot_table_submissions'],
            fmt(r['diagnostic_over_primary_rate'])) for r in d['perturbations']))
    section('曲线：每次原始点与中位数（非热图）')
    for filename in figures:
        path = out / (filename + '.png')
        encoded = base64.b64encode(path.read_bytes()).decode('ascii')
        lines.append('<figure><img alt="' + html.escape(filename) + '" src="data:image/png;base64,' +
                     encoded + '"><figcaption>' + html.escape(filename) + '</figcaption></figure>')
    section('真实性、来源与边界')
    paragraph('主实验二进制、构建命令、数据库源、Rust源码、动态库和所有被包含头文件均在prepared.json哈希闭包中；sources目录归档计时所用代码。plotting_source_sha256单列，不冒充measurement_source_sha256。')
    paragraph('限定：node1固定核且无SMT兄弟/软件超额订阅；32GiB虚拟地址上限、每worker400万次插入保护上限，触顶属于失败而非有效截尾。全库游标精确集合、完整性、状态、缓冲区、joined teardown必须通过。')
    for caveat in manifest['caveats']:
        paragraph(caveat)
    paragraph('原始根与历史169/180容量失败、169/180控制器旧阈值拒绝、180/180最终多表实验彼此独立；这里没有复用或改写其原始数据。原文关于结合摊销/缓存迁移的解释只是机制假设，不是本研究直接测得的归因。')
    paragraph(f'原始manifest SHA256: {summary["manifest_sha256"]}；prepared SHA256: {summary["prepared_sha256"]}；计时源码: {summary["measurement_source_sha256"]}；当前绘图源码: {summary["plotting_source_sha256"]}。')
    failures = ', '.join(t['id'] + '=' + t['status'] + ':' + t.get('error', '')
                         for t in p['trials'] + d['trials'] if t['status'] != 'ok')
    paragraph('失败/缺失全列表：' + (failures or '无'))
    (out / 'report.html').write_text('<!doctype html><html lang="zh"><meta charset="utf-8"><title>UpScaleDB高争用</title><style>body{font:16px/1.6 sans-serif;max-width:1400px;margin:2em auto;padding:1em}table{border-collapse:collapse}td,th{border:1px solid #aaa;padding:.35em} .scroll{overflow:auto}img{max-width:100%}figure{margin:2em 0}p{overflow-wrap:anywhere}</style><body>' + '\n'.join(lines) + '</body></html>')


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input-root', type=Path, required=True)
    parser.add_argument('--analysis-root', type=Path, help='fresh output directory for a revised presentation')
    args = parser.parse_args()
    print(analyze(args.input_root.expanduser().resolve(),
                  args.analysis_root.expanduser().resolve() if args.analysis_root else None))
