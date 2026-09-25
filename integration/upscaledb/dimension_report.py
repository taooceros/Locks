#!/usr/bin/env python3
"""Render the completed joined UpScaleDB study by dimension; never run trials.

From the repository root:
  devenv shell -- python3 integration/upscaledb/dimension_report.py \
    --input-root .worktree/upscaledb-joined-scaling \
    --output-dir docs/reports/upscaledb-joined-dimensions
"""
import argparse
import base64
import hashlib
import html
import json
import math
from pathlib import Path


VARIANTS = ('native', 'fc', 'fc_pq', 'uscl', 'cfl_local')
LABELS = {'native': 'Native', 'fc': 'FC', 'fc_pq': 'FC-PQ',
          'uscl': 'USCL', 'cfl_local': 'CFL-local'}
COLORS = {'native': '#414951', 'fc': '#0072B2', 'fc_pq': '#D55E00',
          'uscl': '#009E73', 'cfl_local': '#A05EB5'}
PHYSICAL = tuple(f'fixed-physical-compact-w{w}' for w in (1, 2, 4, 8, 16, 32, 64))
BALANCED = ('fixed-physical-balanced-w8', 'fixed-physical-balanced-w32')
SMT = tuple(f'fixed-smt-compact-c{c}-w{2*c}' for c in (4, 16, 32, 64))
SMT_BALANCED = 'fixed-smt-balanced-c16-w32'
NUMA = ('fixed-numa-remote-socket1-w8-bind0',
        'fixed-numa-compact-w8-interleave', 'fixed-numa-balanced-w8-interleave')
DURATION = ('duration-physical-compact-w8', 'duration-physical-balanced-w8',
            'duration-smt-compact-c4-w8', 'duration-physical-compact-w64',
            'duration-smt-compact-c64-w128')
FIXED = PHYSICAL + BALANCED + SMT + (SMT_BALANCED,) + NUMA
CASE_LABELS = dict(zip(FIXED,
    ('P1', 'P2', 'P4', 'P8', 'P16', 'P32', 'P64', 'B8', 'B32',
     'S4/8', 'S16/32', 'S32/64', 'S64/128', 'SB16/32', 'R8', 'IC8', 'IB8')))
CASE_LABELS.update(dict(zip(DURATION, ('P8', 'B8', 'S4/8', 'P64', 'S64/128'))))
METRICS = ('elapsed_s', 'find_ops_s', 'insert_ops_s', 'total_ops_s',
           'timed_process_cpu_s', 'writer_max_no_progress_s',
           'find_latency_mean_ns', 'insert_latency_mean_ns',
           'find_latency_p99_upper_ns', 'insert_latency_p99_upper_ns')
CONTRAST_PAIRS = (
    ('physical balanced/compact W=8', PHYSICAL[3], BALANCED[0]),
    ('physical balanced/compact W=32', PHYSICAL[5], BALANCED[1]),
    ('SMT/physical same workers W=8', PHYSICAL[3], SMT[0]),
    ('SMT/physical same workers W=32', PHYSICAL[5], SMT[1]),
    ('SMT/physical same workers W=64', PHYSICAL[6], SMT[2]),
    ('SMT/physical same cores C=4', PHYSICAL[2], SMT[0]),
    ('SMT/physical same cores C=16', PHYSICAL[4], SMT[1]),
    ('SMT/physical same cores C=32', PHYSICAL[5], SMT[2]),
    ('SMT/physical same cores C=64', PHYSICAL[6], SMT[3]),
    ('SMT balanced/compact C=16 W=32', SMT[1], SMT_BALANCED),
    ('remote/local W=8 bind0', PHYSICAL[3], NUMA[0]),
    ('interleave/bind0 compact W=8', PHYSICAL[3], NUMA[1]),
    ('interleave/bind0 balanced W=8', BALANCED[0], NUMA[2]),
)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_json(path):
    def reject(value):
        raise ValueError(f'{path}: nonfinite JSON value: {value}')
    return json.loads(path.read_text(encoding='utf-8'), parse_constant=reject)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def load_data(root):
    overview_path = root / 'overview' / 'scaling.json'
    data = load_json(overview_path)
    require(data['schema'] == 1, 'Unsupported scaling.json schema')
    study = root / 'study.json'
    require(digest(study) == data['study_sha256'], 'Study SHA256 differs from scaling.json')
    expected_cases = set(FIXED + DURATION)
    cases = {c['name']: c for c in data['cases']}
    require(len(data['cases']) == 22 and set(cases) == expected_cases,
            'Expected exactly the 17 fixed and 5 sustained joined-study cases')
    for name, case in cases.items():
        config = case['config']
        mode = 'fixed' if name in FIXED else 'duration'
        require(config['mode'] == mode and config['repetitions'] == 3
                and set(config['variants']) == set(VARIANTS)
                and config['workers'] == case['placement']['workers'],
                f'{name}: mode/repetitions/variants/workers mismatch')
        require(config['reads'] == 400000 and config['inserts'] == 400000
                and config['warmup'] == .5 and config['seconds'] == 10,
                f'{name}: unexpected workload; revise report methods before plotting')
    sources = data['sources']
    require(len(sources) == 22 and {s['case'] for s in sources} == expected_cases,
            'Expected one source summary per case')
    for source in sources:
        path = root / source['path']
        require(path.resolve().is_relative_to(root.resolve()),
                f'Unsafe summary path: {source["path"]}')
        require(digest(path) == source['sha256'], f'{path}: summary SHA256 mismatch')
    expected_rows = {(case, variant, metric)
                     for case in expected_cases for variant in VARIANTS
                     for metric in METRICS + (('paired_native_elapsed_speedup',)
                         if case in FIXED else ('paired_native_find_throughput_ratio',
                                                'paired_native_insert_throughput_ratio'))}
    rows = {}
    for row in data['rows']:
        key = (row['case'], row['variant'], row['metric'])
        require(key not in rows, f'Duplicate metric row: {key}')
        require(row['n'] == 3 and all(isinstance(row[k], (int, float))
                and math.isfinite(row[k]) for k in ('min', 'mean', 'max'))
                and 0 <= row['min'] <= row['mean'] <= row['max'],
                f'Incomplete/nonfinite/out-of-order n=3 range: {key}')
        require(row['mode'] == ('fixed' if row['case'] in FIXED else 'duration'),
                f'Wrong mode: {key}')
        rows[key] = row
    require(set(rows) == expected_rows,
            f'Missing/unexpected metric rows: missing={sorted(expected_rows - set(rows))}; '
            f'extra={sorted(set(rows) - expected_rows)}')
    expected_contrasts = {(label, variant, metric)
        for label, _, _ in CONTRAST_PAIRS for variant in VARIANTS
        for metric in ('total_ops_s', 'timed_process_cpu_s',
                       'find_latency_mean_ns', 'insert_latency_mean_ns')}
    contrasts = {}
    pairs = {label: (reference, alternative) for label, reference, alternative in CONTRAST_PAIRS}
    for contrast in data['placement_contrasts']:
        key = (contrast['comparison'], contrast['variant'], contrast['metric'])
        require(key not in contrasts and key[0] in pairs,
                f'Duplicate/unknown placement contrast: {key}')
        first, second = pairs[key[0]]
        require((contrast['reference_case'], contrast['alternative_case']) == (first, second),
                f'Wrong direction in contrast: {key}')
        a = rows[(first, key[1], key[2])]['mean']
        b = rows[(second, key[1], key[2])]['mean']
        require(a > 0 and b > 0 and math.isclose(contrast['alternative_over_reference'], b/a,
                                                 rel_tol=1e-10),
                f'Contrast is not alternative/reference ratio of case means: {key}')
        contrasts[key] = contrast
    require(set(contrasts) == expected_contrasts,
            f'Missing/unexpected placement contrasts: missing={sorted(expected_contrasts - set(contrasts))}; '
            f'extra={sorted(set(contrasts) - expected_contrasts)}')
    return data, rows, contrasts, overview_path


def point(rows, case, variant, metric):
    return rows[(case, variant, metric)]


def mean(rows, case, variant, metric):
    return point(rows, case, variant, metric)['mean']


def contrast(contrasts, label, variant):
    return contrasts[(label, variant, 'total_ops_s')]['alternative_over_reference']


def setup_plotting():
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    plt.rcParams.update({'font.size': 10, 'axes.titlesize': 12,
                         'axes.labelsize': 10, 'legend.fontsize': 9,
                         'savefig.facecolor': 'white', 'axes.spines.top': False,
                         'axes.spines.right': False})
    return plt


def make_figure(plt, title, panels=1, width=15.5, height=5.9, note=''):
    fig, axes = plt.subplots(1, panels, figsize=(width, height), squeeze=False)
    fig.suptitle(title, fontsize=15, y=.98)
    if note:
        fig.text(.5, .117, note, ha='center', va='center', fontsize=9, color='#4b5563')
    fig.subplots_adjust(left=.085, right=.975, bottom=.22, top=.85, wspace=.27)
    return fig, axes[0]


def finish(plt, fig, path, legend=True):
    if legend:
        handles, labels = fig.axes[0].get_legend_handles_labels()
        fig.legend(handles, labels, loc='lower center', ncol=min(len(handles), 5),
                   frameon=False, bbox_to_anchor=(.5, .025))
    fig.savefig(path, dpi=155)
    plt.close(fig)


def series(ax, categories, labels, variants, measure, *, ranges=True, log=False,
           ylabel='', rotate=0, reference=None, scale=1.):
    # Offsets separate n=3 observed ranges without falsely pairing case means.
    for index, variant in enumerate(variants):
        shift = (index - (len(variants)-1)/2) * (.145 if len(categories) > 7 else .11)
        records = [measure(case, variant) for case in categories]
        values = [record['mean'] * scale for record in records]
        if log:
            require(all(value > 0 for value in values), f'Log axis needs positive {ylabel}')
        errors = None
        if ranges:
            errors = [[(record['mean']-record['min'])*scale for record in records],
                      [(record['max']-record['mean'])*scale for record in records]]
        ax.errorbar([i+shift for i in range(len(categories))], values, yerr=errors,
                    fmt='o-', markersize=4.3, linewidth=1.15, capsize=2.2,
                    elinewidth=.85, color=COLORS[variant], label=LABELS[variant])
    ax.set_xticks(range(len(categories)), labels, rotation=rotate,
                  ha='right' if rotate else 'center')
    ax.set_xlim(-.6, len(categories)-.4)
    ax.set_ylabel(ylabel)
    if log:
        ax.set_yscale('log')
    if reference is not None:
        ax.axhline(reference, color='#777', linestyle='--', linewidth=1, zorder=0)
    ax.grid(axis='y', alpha=.23, which='both')


def contrast_series(ax, contrasts, pairs, labels, variants=VARIANTS):
    for variant in variants:
        values = [contrast(contrasts, name, variant) for name in pairs]
        require(all(x > 0 for x in values), 'Nonpositive placement ratio')
        ax.plot(range(len(pairs)), values, 'o-', label=LABELS[variant],
                color=COLORS[variant], markersize=5, linewidth=1.2)
    ax.axhline(1, color='#777', linestyle='--', linewidth=1)
    ax.set_yscale('log')
    ax.set_xticks(range(len(pairs)), labels)
    ax.set_ylabel('Alternative / reference throughput (×); higher favors alternative')
    ax.grid(axis='y', which='both', alpha=.25)


def draw_all(rows, contrasts, folder):
    plt = setup_plotting()
    files = []

    def save(fig, name, legend=True):
        path = folder / f'{name}.png'
        finish(plt, fig, path, legend=legend)
        files.append(path)

    fig, axes = make_figure(plt, '01  Physical compact: absolute scaling and own-baseline retention',
        2, note='Fixed 400k find + 400k insert; n=3 observed min–max on throughput only; higher is better.')
    series(axes[0], PHYSICAL, [str(w) for w in (1,2,4,8,16,32,64)], VARIANTS,
           lambda c,v: point(rows,c,v,'total_ops_s'), log=True,
           ylabel='Completed operations / s (fixed work)')
    axes[0].set_xlabel('Physical cores = workers; node-0 bind')
    for v in VARIANTS:
        base = mean(rows, PHYSICAL[0], v, 'total_ops_s')
        values = [mean(rows,c,v,'total_ops_s') / base for c in PHYSICAL]
        axes[1].plot(range(7), values, 'o-', color=COLORS[v], label=LABELS[v])
    axes[1].axhline(1, color='#777', linestyle='--', linewidth=1)
    axes[1].set_xticks(range(7), [str(w) for w in (1,2,4,8,16,32,64)])
    axes[1].set_xlabel('Physical cores = workers (64 spans both sockets)')
    axes[1].set_ylabel('Throughput / own W1 throughput (×); higher is better')
    axes[1].set_yscale('log')
    axes[1].grid(axis='y', which='both', alpha=.23)
    save(fig, '01_physical_scaling')

    fig, axes = make_figure(plt, '02  Fixed-work paired elapsed-time speedup vs Native',
        width=17.5, height=6.9,
        note='Each dot is mean of 3 within-block Native/variant elapsed ratios; whiskers = observed min–max, NOT CI.')
    series(axes[0], FIXED, [CASE_LABELS[c] for c in FIXED], VARIANTS[1:],
        lambda c,v: point(rows,c,v,'paired_native_elapsed_speedup'),
        ylabel='Native elapsed / variant elapsed (×); higher is faster',
        rotate=45, reference=1, log=True)
    save(fig, '02_fixed_paired_speedup')

    fig, axes = make_figure(plt, '03  NUMA: physical placement and explicit memory policies', 2,
        width=16, height=6.1,
        note='Fixed work; points divide serially run case means (n=3 each). No paired-placement whiskers or causal NUMA claim.')
    names = [CONTRAST_PAIRS[i][0] for i in (0,1)]
    contrast_series(axes[0], contrasts, names, ('B8/P8', 'B32/P32'))
    axes[0].set_title('Node-0 bind: balanced / compact physical')
    names = [CONTRAST_PAIRS[i][0] for i in (10,11,12)]
    contrast_series(axes[1], contrasts, names, ('R8/P8', 'IC8/P8', 'IB8/B8'))
    axes[1].set_title('Remote bind0 or interleave / reference bind0')
    save(fig, '03_numa_placement_memory')

    fig, axes = make_figure(plt, '04  SMT: same workers is NOT same physical-core budget', 2,
        width=16, height=6.1,
        note='Fixed work; ratios of case means, no paired-placement intervals; 2 workers/core on different logical CPUs.')
    names = [CONTRAST_PAIRS[i][0] for i in (2,3,4,9)]
    contrast_series(axes[0], contrasts, names, ('W8:4/8', 'W32:16/32',
                                                  'W64:32/64', 'SB32/S32'))
    axes[0].set_title('Same W: fewer cores; last point = balanced SMT / compact SMT')
    names = [CONTRAST_PAIRS[i][0] for i in (5,6,7,8)]
    contrast_series(axes[1], contrasts, names, ('C4:8/4', 'C16:32/16',
                                                  'C32:64/32', 'C64:128/64'))
    axes[1].set_title('Same C: add siblings, doubling workers + contention')
    save(fig, '04_smt_two_budgets')

    fig, axes = make_figure(plt, '05  Sustained absolute throughput: find and insert', 2,
        width=16, height=6.1,
        note='10 s requested window; n=3 observed min–max; per-operation rates, not fixed-work speedups.')
    for ax, op in zip(axes, ('find', 'insert')):
        series(ax, DURATION, [CASE_LABELS[c] for c in DURATION], VARIANTS,
               lambda c,v,o=op: point(rows,c,v,f'{o}_ops_s'), log=True,
               ylabel=f'{op.capitalize()} completed operations / s; higher is better')
        ax.set_title(f'{op.capitalize()} (own on-time completions / requested 10 s)')
    save(fig, '05_duration_find_insert')

    for number, mode, cases, kind, name in (
        (6, 'fixed', PHYSICAL, 'mean', '06_fixed_mean_latency'),
        (7, 'fixed', PHYSICAL, 'p99_upper', '07_fixed_p99_latency'),
        (8, 'duration', DURATION, 'mean', '08_duration_mean_latency'),
        (9, 'duration', DURATION, 'p99_upper', '09_duration_p99_latency')):
        title_kind = 'mean' if kind == 'mean' else 'pooled-worker histogram p99 bucket upper bound'
        fig, axes = make_figure(plt,
            f'{number:02d}  {mode.capitalize()} {title_kind}: find and insert', 2,
            width=16, height=6.1,
            note='n=3 trial-summary min–max; histogram p99 is an upper bucket bound, not per-worker tail or fairness.'
                 if kind == 'p99_upper' else
                 'n=3 trial-summary min–max; measured operation latency, not elapsed-case time.')
        labels = ([str(w) for w in (1,2,4,8,16,32,64)] if mode == 'fixed'
                  else [CASE_LABELS[c] for c in cases])
        for ax, op in zip(axes, ('find', 'insert')):
            series(ax, cases, labels, VARIANTS,
                   lambda c,v,o=op,k=kind: point(rows,c,v,f'{o}_latency_{k}_ns'),
                   scale=.001, log=True,
                   ylabel=f'{op.capitalize()} latency (µs); lower is better')
            ax.set_title(f'{op.capitalize()} {title_kind}')
        save(fig, name)

    fig, axes = make_figure(plt, '10  Fixed-work timed process CPU cost', width=17.5, height=6.9,
        note='n=3 observed min–max; same 800k completed operations per case; CPU seconds are NOT joules.')
    series(axes[0], FIXED, [CASE_LABELS[c] for c in FIXED], VARIANTS,
           lambda c,v: point(rows,c,v,'timed_process_cpu_s'),
           log=True, rotate=45, ylabel='Timed process CPU (CPU-s); lower is better')
    save(fig, '10_fixed_cpu')

    fig, axes = make_figure(plt, '11  Ten-second-window timed process CPU cost',
        width=13.5, note='n=3 observed min–max; backends perform unequal work in duration mode; not energy or CPU/op.')
    series(axes[0], DURATION, [CASE_LABELS[c] for c in DURATION], VARIANTS,
           lambda c,v: point(rows,c,v,'timed_process_cpu_s'),
           log=True, ylabel='Timed process CPU (CPU-s); lower is better')
    save(fig, '11_duration_cpu')

    fig, axes = make_figure(plt, '12  Writer progress observation and FC / FC-PQ operation mix',
        2, width=16, height=6.1,
        note='Left: n=3 observed trial range, NOT a starvation bound. Right: absolute case means; CPU in Fig. 11.')
    series(axes[0], DURATION, [CASE_LABELS[c] for c in DURATION], VARIANTS,
           lambda c,v: point(rows,c,v,'writer_max_no_progress_s'),
           scale=1000, log=True, ylabel='Worst writer no-progress gap (ms); lower is better')
    axes[0].set_title('Maximum interval observed per trial, then 3-trial mean')
    for case in DURATION:
        x = [mean(rows,case,v,'find_ops_s') for v in ('fc','fc_pq')]
        y = [mean(rows,case,v,'insert_ops_s') for v in ('fc','fc_pq')]
        axes[1].plot(x, y, '-', color='#aaaaaa', linewidth=1, zorder=1)
        axes[1].annotate(CASE_LABELS[case], (x[0],y[0]), xytext=(4,4),
                         textcoords='offset points', fontsize=8)
        for i,v in enumerate(('fc','fc_pq')):
            axes[1].scatter(x[i],y[i], s=47, color=COLORS[v], zorder=3)
    axes[1].set_xscale('log')
    axes[1].set_yscale('log')
    axes[1].set_xlabel('Find completed operations / s; higher is better')
    axes[1].set_ylabel('Insert completed operations / s; higher is better')
    axes[1].set_title('FC (blue) → FC-PQ (orange), same case; case means')
    axes[1].grid(alpha=.2, which='both')
    save(fig, '12_writer_gap_fc_tradeoff')
    return files


def table_markdown(headers, body):
    return ('| ' + ' | '.join(headers) + ' |\n| ' + ' | '.join('---' for _ in headers)
            + ' |\n' + ''.join('| ' + ' | '.join(map(str, row)) + ' |\n' for row in body))


def table_html(headers, body):
    cell = lambda name, tag: f'<{tag}>{html.escape(str(name))}</{tag}>'
    return ('<div class="table-scroll"><table><thead><tr>'
            + ''.join(cell(x, 'th') for x in headers) + '</tr></thead><tbody>'
            + ''.join('<tr>' + ''.join(cell(x, 'td') for x in row) + '</tr>' for row in body)
            + '</tbody></table></div>')


def build_sections(rows, contrasts):
    p = lambda c,v,m: mean(rows,c,v,m)
    r = lambda label,v: contrast(contrasts,label,v)
    fixed64, dur64, dur128 = PHYSICAL[-1], DURATION[-2], DURATION[-1]
    fixed_table = []
    dur_table = []
    for v in VARIANTS:
        fixed_table.append((LABELS[v], f'{p(fixed64,v,"total_ops_s"):,.0f}',
          f'{p(fixed64,v,"timed_process_cpu_s"):.2f}',
          f'{p(fixed64,v,"find_latency_mean_ns")/1000:.1f}',
          f'{p(fixed64,v,"insert_latency_mean_ns")/1000:.1f}',
          f'{p(fixed64,v,"paired_native_elapsed_speedup"):.2f}'))
        dur_table.append((LABELS[v], f'{p(dur64,v,"find_ops_s"):,.0f}',
          f'{p(dur64,v,"insert_ops_s"):,.0f}',
          f'{p(dur64,v,"timed_process_cpu_s"):.2f}',
          f'{p(dur64,v,"writer_max_no_progress_s")*1000:.3f}'))
    sections = []
    def add(title, paragraphs, figure=None, caption=None, table=None):
        sections.append({'title': title, 'paragraphs': paragraphs, 'figure': figure,
                         'caption': caption, 'table': table})
    add('方法、对象与统计口径', [
        '研究对象是经过显式调用方 join、独占销毁约束的受限 UpScaleDB 内存 Store：64 位、8 字节键与 8 字节记录；无事务、恢复、重复键、压缩或直接访问。find 在预装载的 100,000 键域内确定性取键，insert 使用不重叠的唯一键域。比较 Native、FC、FC-PQ、USCL 与 CFL-local 五个完整端到端实现，而非分离测量调度器成本。',
        '来自已完成的 joined study：17 个 fixed case × 5 后端 × 3 次 = 255 次，加 5 个 duration case × 5 后端 × 3 次 = 75 次，共 330 次，无主试验排除。随机化每块内后端顺序，种子 20260923；每次新进程，0.5 秒一次性 Store 预热。fixed 精确完成 400,000 次 find + 400,000 次 insert；duration 请求 10 秒窗口，按截止前完成数除以请求的 10 秒分别计算 find / insert 速率。预装载数 100,000；200,000,000 唯一 insert 为安全上限，不是工作量。每个数据点是三次试验汇总的均值，若画须线则是实际最小—最大值而非置信区间。',
        '每个 trial 的操作延迟直方图先合并各 worker，再在 case 级对三个 trial 的延迟均值和 p99 桶上界分别取均值；这些不是跨三个 trial 重新合并的 p99，也不是逐线程最坏值。直方图可能包括截止后完成的工作，而 duration 速率只计截止前完成数；二者窗口不完全相同。timed_process_cpu_s 是 harness 定时工作段的进程累计 CPU 时间，不是整个 runner 生命周期，亦非能耗。特别不把时长模式的边际均值除出来称为精确的单次 CPU/操作。',
        '物理 compact 使用每核一个逻辑 CPU；64 核跨两个 socket。W1 由唯一 worker 交替执行两种操作，多 worker 则为均衡的专职 finder/inserter；W1 仅作同一后端吞吐留存的描述性基准，不能假装工作角色恒定。NUMA 对照在固定 worker 数下区分物理 compact 与 balanced、node-0 bind / node-0 远端 socket-1 执行 / 两节点 interleave；SMT 对照区分同 W 与同物理核心 C 两种不等价预算。绑定/页分布是进程整体性质，非数据库页单独因果实验；一核一线程也不等于 BIOS 关闭 SMT。'],
        table=(('固定工作 P64：后端','吞吐 (ops/s)','定时 CPU (CPU-s)',
                'find 均值 (µs)','insert 均值 (µs)','配对耗时加速 (×)'), fixed_table))
    add('01 物理核心扩展与同后端留存', [
        f'固定工作下，P64 的 FC 均值为 {p(fixed64,"fc","total_ops_s"):,.0f} ops/s，'
        f'Native 为 {p(fixed64,"native","total_ops_s"):,.0f} ops/s；FC-PQ 为 '
        f'{p(fixed64,"fc_pq","total_ops_s"):,.0f} ops/s。右图仅把每个后端除以它自己的 P1 均值：'
        f'FC 的 P64/P1={p(fixed64,"fc","total_ops_s")/p(PHYSICAL[0],"fc","total_ops_s"):.2f}×，'
        f'FC-PQ={p(fixed64,"fc_pq","total_ops_s")/p(PHYSICAL[0],"fc_pq","total_ops_s"):.2f}×。'
        '相对 Native 的优势不是线性强扩展；P1 混合角色与 P64 两 socket 的执行配置不同，也不能从吞吐留存推出核心效率或缓存因果解释。'],
        '01_physical_scaling.png', '左：fixed 完成总操作/s，均值与三次观测范围；右：各后端对自身 P1 均值的留存，只有描述性点值，非配对比值误差条。')
    add('02 固定工作 Native 配对耗时加速', [
        f'P8/ P32/ P64 compact 的 FC 相对 Native 的同块配对耗时加速均值依次为 '
        f'{p(PHYSICAL[3],"fc","paired_native_elapsed_speedup"):.2f}×、'
        f'{p(PHYSICAL[5],"fc","paired_native_elapsed_speedup"):.2f}×、'
        f'{p(fixed64,"fc","paired_native_elapsed_speedup"):.2f}×；P64 FC-PQ 为 '
        f'{p(fixed64,"fc_pq","paired_native_elapsed_speedup"):.2f}×。每个速度点来自已经汇总的同一个随机化 block 内 Native 耗时 / 后端耗时的三次比值；不从耗时边际均值重新推导逐次配对比值。'
        '与吞吐比/持续模式比值是不同统计量；W1 的工作角色也不同。'],
        '02_fixed_paired_speedup.png',
        'P=物理 compact；B=物理 balanced；S=C/W compact SMT；SB=balanced SMT；R=socket1 远端 bind0；IC/IB=compact/balanced interleave。横线 1× 为 Native 基准。')
    add('03 NUMA 物理分布及内存策略', [
        f'在固定工作 P8 上 balanced/compact 的 FC 总吞吐均值比 {r("physical balanced/compact W=8","fc"):.2f}×，'
        f'P32 为 {r("physical balanced/compact W=32","fc"):.2f}×；'
        f'P8 socket1 远端执行 / socket0 本地执行、两者 bind0 的 FC 比值却为 '
        f'{r("remote/local W=8 bind0","fc"):.2f}×。'
        '远端配置并非普遍变慢：核心集合、socket 身份、初始化、串行运行次序与页分布可能混杂。'
        'interleave/bind0 的 compact 与 balanced 比较分别保持对应 CPU 配置，不能当作纯内存访问延迟测量。'],
        '03_numa_placement_memory.png',
        'B8/P8、B32/P32：同 W 跨 socket/compact；R8/P8：socket1 执行 bind0 / socket0 执行 bind0；IC8/P8 与 IB8/B8：各自 interleave / bind0。全图均为两组 case 均值的比，而非跨 placement 配对试验。')
    add('04 SMT：同工人数与同核心预算', [
        f'同 W8，SMT 4 核/8 工人对物理 8 核/8 工人的 FC 比 {r("SMT/physical same workers W=8","fc"):.2f}×；'
        f'同 C64，把物理 64 工人增加为 SMT 128 工人时，FC 比 {r("SMT/physical same cores C=64","fc"):.2f}×，'
        f'USCL 比 {r("SMT/physical same cores C=64","uscl"):.2f}×。同 C 比较同时增加了工人数与竞争强度；'
        '同 W 比较却减少了核心资源。同 W32 的 SB32/S32 比较只改变 SMT 工人跨 socket 的分布。'
        '不能把这两种比值合并为“SMT 加速”或推断独占核心及宿主机无干扰。'],
        '04_smt_two_budgets.png',
        '左三组 S/P 是同工人数，第四组 SB32/S32 是 balanced/compact SMT；右四组是固定物理核心 C，启用同核 sibling 后工人数翻倍。均为 case 均值比，无推测范围。')
    add('05 十秒窗口 find 与 insert 的绝对速率', [
        f'P64 上 FC find={p(dur64,"fc","find_ops_s"):,.0f}、insert={p(dur64,"fc","insert_ops_s"):,.0f} ops/s；'
        f'FC-PQ find={p(dur64,"fc_pq","find_ops_s"):,.0f}、insert={p(dur64,"fc_pq","insert_ops_s"):,.0f} ops/s。'
        '分别报告两类完成率以免混合总量改变工作组成时掩盖写入进度；duration 模式不能与固定 800,000 操作的完成时间混作同一速度指标。'],
        '05_duration_find_insert.png', '五个 sustained case：P8、B8、S4/8、P64、S64/128；左右分别为 find、insert 截止前完成数/请求 10 秒。',
        table=(('十秒 P64：后端','find (ops/s)','insert (ops/s)',
                '定时 CPU (CPU-s)','writer gap (ms)'), dur_table))
    add('06 固定工作操作平均延迟', [
        f'P64 find/insert 平均延迟：FC '
        f'{p(fixed64,"fc","find_latency_mean_ns")/1000:.1f}/'
        f'{p(fixed64,"fc","insert_latency_mean_ns")/1000:.1f} µs；FC-PQ '
        f'{p(fixed64,"fc_pq","find_latency_mean_ns")/1000:.1f}/'
        f'{p(fixed64,"fc_pq","insert_latency_mean_ns")/1000:.1f} µs。'
        '读写分图、对数纵轴同时展示五后端，避免吞吐提升即读写均延迟都改善的误读。单点为 trial 摘要均值再取三次均值；非某单一 worker 的服务时间。'],
        '06_fixed_mean_latency.png', '固定工作 P1–P64，find 与 insert 各自的平均操作延迟（µs），低更好；观测范围非置信区间。')
    add('07 固定工作直方图 p99 上界', [
        'p99_upper 表示所在指数直方图桶的上界，不等于精确的原始样本分位数。三个 trial 各自先汇总 worker 的操作分布，再汇总各 trial 的 p99 上界。'
        '小于 1% 的极长等待可以显著抬高均值，使均值大于 p99 桶上界而不矛盾；汇总操作直方图不能代表单个最慢 worker，更不能推导服务公平性。'],
        '07_fixed_p99_latency.png', '固定工作下读/写 p99 桶上界（µs），低更好；误差线是三次 trial 上界观测范围。')
    add('08 持续运行的操作平均延迟', [
        f'P64 sustained USCL 的 find / insert 均值分别为 '
        f'{p(dur64,"uscl","find_latency_mean_ns")/1000:.1f} / '
        f'{p(dur64,"uscl","insert_latency_mean_ns")/1000:.1f} µs。'
        '这些操作延迟的统计样本范围与截止前速率的计数范围并不严格相同：直方图可能包含截止后工作；必须同时参照吞吐、尾部和 writer gap，不能单凭平均值选“最公平”后端。'],
        '08_duration_mean_latency.png', '五个持续模式 case，分别展示读/写平均操作延迟（µs）。')
    add('09 持续运行的 p99 桶上界', [
        f'P64 USCL 的 find / insert p99 桶上界均值分别为 '
        f'{p(dur64,"uscl","find_latency_p99_upper_ns")/1000:.3f} / '
        f'{p(dur64,"uscl","insert_latency_p99_upper_ns")/1000:.3f} µs；'
        f'同一组的 writer 最长无进展间隔均值为 {p(dur64,"uscl","writer_max_no_progress_s")*1000:.3f} ms。'
        '操作直方图 pooled-worker p99 小于平均值在小概率超长尾存在时完全可能；p99 既不是每位 writer 的最坏暂停，也不是无饥饿保证。'],
        '09_duration_p99_latency.png', '五个持续模式 case，find/insert 分面；直方图 p99 桶上界、n=3 的实际范围，低更好。')
    add('10 固定工作 CPU 成本', [
        f'同为固定 P64 的 800,000 操作，FC 定时进程 CPU 均值 '
        f'{p(fixed64,"fc","timed_process_cpu_s"):.2f} CPU-s，FC-PQ '
        f'{p(fixed64,"fc_pq","timed_process_cpu_s"):.2f} CPU-s，USCL '
        f'{p(fixed64,"uscl","timed_process_cpu_s"):.2f} CPU-s。'
        'FC-PQ 此处总吞吐低于 FC、CPU 时间高于 FC；这是整体版本对比，不能直接归因于优先队列独立开销。CPU 时间是所有定时运行线程累计，并非墙钟秒或能量。'],
        '10_fixed_cpu.png', '固定工作 17 case 的 timed process CPU (CPU-s)，均值及实际三次范围；数值越低仅表示本指标更小。')
    add('11 十秒模式 CPU 成本', [
        f'P64 duration FC / FC-PQ 的定时 CPU 分别为 '
        f'{p(dur64,"fc","timed_process_cpu_s"):.2f} / '
        f'{p(dur64,"fc_pq","timed_process_cpu_s"):.2f} CPU-s；USCL '
        f'{p(dur64,"uscl","timed_process_cpu_s"):.2f} CPU-s。'
        '这些后端在相同请求窗口内完成数量不同；duration CPU 总秒数不能与固定工作 CPU 秒数跨模式直接比较，也不能当作统一工作量的效率、功耗或能源。'],
        '11_duration_cpu.png', '五个 duration case 的 timed process CPU (CPU-s)，低值不直接意味着高吞吐或低写入停顿。')
    add('12 写入进展及 FC / FC-PQ 权衡', [
        f'P64 duration 中每 trial 最差 writer 无进展间隔再取三次均值，FC '
        f'{p(dur64,"fc","writer_max_no_progress_s")*1000:.3f} ms、FC-PQ '
        f'{p(dur64,"fc_pq","writer_max_no_progress_s")*1000:.3f} ms、USCL '
        f'{p(dur64,"uscl","writer_max_no_progress_s")*1000:.3f} ms。'
        '这些是有限观察窗口的最大间隔均值，绝非饥饿时间上界；更低 CPU 不蕴含更低写入停顿。',
        f'FC-PQ 的 P64 find/FC 比为 {p(dur64,"fc_pq","find_ops_s")/p(dur64,"fc","find_ops_s"):.2f}×，'
        f'insert/FC 比为 {p(dur64,"fc_pq","insert_ops_s")/p(dur64,"fc","insert_ops_s"):.2f}×；'
        f'S64/128 对应 find {p(dur128,"fc_pq","find_ops_s")/p(dur128,"fc","find_ops_s"):.2f}×、'
        f'insert {p(dur128,"fc_pq","insert_ops_s")/p(dur128,"fc","insert_ops_s"):.2f}×。'
        '这里为便于解释的两版本 case 均值比，不是新造的逐 trial 配对比；若需 Native 基准逐次范围应使用 overview 中现成的 paired_native_find/insert_throughput_ratio。结合图 11 的 CPU 成本，不能只报 find 增幅就宣称全面改善。'],
        '12_writer_gap_fc_tradeoff.png', '左：最大 writer 无进展间隔 (ms)，仅已观测最大值；右：五个 case 中 FC→FC-PQ 的 find/insert 吞吐均值移动，灰线仅连接同一 case，不表示随时间演化。')
    add('解释边界与来源', [
        '主试验未开启 callback service profiling；单独的 profile smoke 是功能验证而非主实验服务公平性证据，因此不提出新的 JFI 或服务公平性主张。不能从 NUMA 比值断定远端内存或缓存一致性因果、从 FC/FC-PQ 整体差异分解队列开销、从 CPU 时间推断能耗。CFL-local 是仓库内的本地代理实现，尚未经论文工件的逐项复现认证；USCL 使用显式相等 1024 worker 权重及既定 TSC 校准假设，不是当代调度器机理复刻。',
        '总样本量小且 sustained 只观察十秒，属于探索性配置对比，非统计显著性或长期饥饿保证。内存策略与 CPU 集通过配置和观测归档，不保证数据库对象页面孤立，也不证明宿主隔离；自动 NUMA balancing 可能存在。原始试验和 build 保存在 .worktree/upscaledb-joined-scaling 等被 Git 忽略的根目录：源代码或本报告提交不是原始证据备份。所有哈希、源 summary 路径、命令、图清单写入 provenance.json；本报告只读取 joined study，绝未混入较早 counted、admission、120 秒研究或 profile smoke。'])
    return sections


def render(sections, files, output, provenance):
    file_names = {p.name for p in files}
    md = ['# UpScaleDB joined study：物理扩展、NUMA、SMT 与进展维度报告',
          '', '报告仅据已完成的 joined cohort；图示指标的较好方向和统计口径见各图题注。', '']
    css = ('body{font:16px/1.75 system-ui,sans-serif;color:#182536;max-width:1180px;'
           'margin:0 auto;padding:1.5rem 2rem;background:#fafbfd}h1,h2{line-height:1.32;'
           'color:#173858}nav{background:#eaf2f7;padding:1rem;border-radius:9px;columns:2}'
           'nav a{display:block;color:#125684;margin:.15rem 0}section{background:white;'
           'padding:1rem 1.7rem;margin:1.4rem 0;border:1px solid #e5ebef;border-radius:8px}'
           'figure{margin:1.2rem 0}img{max-width:100%;height:auto;display:block;margin:auto}'
           'figcaption{color:#4e6070;font-size:.94rem}.table-scroll{overflow:auto}'
           'table{border-collapse:collapse;margin:1rem 0;width:100%;font-size:.93rem}'
           'th,td{padding:.45rem .65rem;border-bottom:1px solid #d9e1e8;text-align:right}'
           'th:first-child,td:first-child{text-align:left}thead{background:#eaf2f7}'
           'code{overflow-wrap:anywhere}p{max-width:100ch}')
    doc = ['<!doctype html><html lang="zh-CN"><head><meta charset="utf-8">',
           '<meta name="viewport" content="width=device-width,initial-scale=1">',
           '<title>UpScaleDB joined study：维度报告</title>', f'<style>{css}</style>',
           '</head><body><header><h1>UpScaleDB joined study：物理扩展、NUMA、SMT 与进展维度报告</h1>',
           '<p>已完成 joined cohort；所有图为非热图。图中的英语短标签避免图形字体依赖，解释与方法为中文。</p></header>',
           '<nav aria-label="章节导航">']
    for index, section in enumerate(sections):
        doc.append(f'<a href="#s{index}">{html.escape(section["title"])}</a>')
    doc.append('</nav><main>')
    for index, section in enumerate(sections):
        title = section['title']
        md.extend([f'## {title}', ''])
        doc.append(f'<section id="s{index}"><h2>{html.escape(title)}</h2>')
        for paragraph in section['paragraphs']:
            md.extend([paragraph, ''])
            doc.append(f'<p>{html.escape(paragraph)}</p>')
        if section['table']:
            headers, body = section['table']
            md.extend([table_markdown(headers, body), ''])
            doc.append(table_html(headers, body))
        if section['figure']:
            name = section['figure']
            require(name in file_names, f'Section missing rendered figure: {name}')
            caption = section['caption']
            md.extend([f'![{title}](figures/{name})', '', f'图注：{caption}', ''])
            embedded = base64.b64encode((output / 'figures' / name).read_bytes()).decode('ascii')
            doc.append('<figure><img loading="lazy" alt="' + html.escape(title, quote=True)
                       + '" src="data:image/png;base64,' + embedded + '"><figcaption>'
                       + html.escape(caption) + '</figcaption></figure>')
        doc.append('</section>')
    md.extend(['## 可复现来源', '',
       f'overview/scaling.json SHA256：`{provenance["overview_sha256"]}`；study.json SHA256：`{provenance["study_sha256"]}`。', '',
       f'汇总源数：{len(provenance["sources"])}；源路径和逐文件 SHA256 均见 [provenance.json](provenance.json)。', '',
       f'生成器 SHA256：`{provenance["generator_sha256"]}`；上游 scaling_report.py SHA256：`{provenance["analysis_script_sha256"]}`。', '',
       '在仓库根目录执行（只生成报告，不重跑测量）：', '', '```sh', provenance['reproduce_command'], '```', ''])
    doc.extend(['</main><footer><h2>可复现来源</h2><p>overview/scaling.json SHA256：<code>',
                provenance['overview_sha256'], '</code>；study.json SHA256：<code>',
                provenance['study_sha256'], '</code>；generator SHA256：<code>',
                provenance['generator_sha256'], '</code>；上游汇总脚本 SHA256：<code>',
                provenance['analysis_script_sha256'], '</code>。逐 case summary SHA256 见',
                ' <a href="provenance.json">provenance.json</a>（HTML 图本身离线可读）。</p>',
                '<p>在仓库根目录运行：</p><pre><code>',
                html.escape(provenance['reproduce_command']),
                '</code></pre></footer></body></html>'])
    (output / 'report.md').write_text('\n'.join(md), encoding='utf-8')
    (output / 'report.html').write_text(''.join(doc), encoding='utf-8')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input-root', type=Path,
                        default=Path('.worktree/upscaledb-joined-scaling'))
    parser.add_argument('--output-dir', type=Path,
                        default=Path('docs/reports/upscaledb-joined-dimensions'))
    args = parser.parse_args()
    root = args.input_root.resolve()
    output = args.output_dir.resolve()
    require(not output.is_relative_to(root), 'Output directory must not overwrite the study')
    data, rows, contrasts, overview_path = load_data(root)
    sections = build_sections(rows, contrasts)
    figure_dir = output / 'figures'
    figure_dir.mkdir(parents=True, exist_ok=True)
    files = draw_all(rows, contrasts, figure_dir)
    require(len(files) == 12, 'Expected all 12 non-heatmap figures')
    command = ('devenv shell -- python3 integration/upscaledb/dimension_report.py '
               '--input-root .worktree/upscaledb-joined-scaling '
               '--output-dir docs/reports/upscaledb-joined-dimensions')
    provenance = {'schema': 1, 'input_root': str(root),
                  'overview_sha256': digest(overview_path),
                  'study_sha256': data['study_sha256'],
                  'analysis_script_sha256': data['analysis_script_sha256'],
                  'generator_sha256': digest(Path(__file__)),
                  'sources': data['sources'], 'figures': [f'figures/{p.name}' for p in files],
                  'reproduce_command': command,
                  'scope': '22 joined cases; 330 primary trials; 3 repetitions per backend per case; no reruns'}
    render(sections, files, output, provenance)
    (output / 'provenance.json').write_text(
        json.dumps(provenance, indent=2, ensure_ascii=False, allow_nan=False) + '\n',
        encoding='utf-8')
    print(f'Wrote {len(files)} figures and report.md/report.html/provenance.json to {output}')


if __name__ == '__main__':
    main()
