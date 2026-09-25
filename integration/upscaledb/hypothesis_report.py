#!/usr/bin/env python3
"""Synthesize saved hypothesis cohorts; never run experiments or alter raw data."""
import argparse
import base64
import hashlib
import html
import json
from pathlib import Path
import shutil
import statistics


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input-root', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    root, out = args.input_root.resolve(), args.output_dir.resolve()
    sources = {}
    data = {}
    for cohort in ('H1', 'H2', 'H3', 'H1-serial', 'H2-serial', 'H3-serial'):
        candidates = (list((root / cohort / 'analysis').glob('generation-*/summary.json'))
                      if cohort.startswith('H1') else [root / cohort / 'analysis/summary.json'])
        if len(candidates) != 1:
            raise ValueError(f'{cohort}: expected exactly one analysis generation')
        path = candidates[0]
        data[cohort] = json.loads(path.read_text())
        sources[cohort] = {'path': str(path), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
    for name, expected in (('H1', 54), ('H1-serial', 18)):
        assert data[name]['valid_trials'] == data[name]['planned_trials'] == expected
        assert not data[name]['failures']
    for name in ('H2', 'H2-serial'):
        assert len(data[name]['trial_summaries']) == 60 and not data[name]['failures']
    for name, expected in (('H3', 36), ('H3-serial', 24)):
        assert data[name]['complete'] and data[name]['successful_trials'] == expected
        assert data[name]['failed_trials'] == data[name]['missing_trials'] == 0
    out.mkdir(parents=True, exist_ok=False)
    (out / 'figures').mkdir()
    md, doc = [], []

    def text(value):
        md.extend([value, ''])
        doc.append('<p>' + html.escape(value) + '</p>')

    def heading(value):
        md.extend(['## ' + value, ''])
        doc.append('<h2>' + html.escape(value) + '</h2>')

    def table(headers, rows):
        md.append('| ' + ' | '.join(headers) + ' |')
        md.append('|' + '|'.join(['---'] * len(headers)) + '|')
        doc.append('<table><tr>' + ''.join('<th>' + html.escape(h) + '</th>' for h in headers) + '</tr>')
        for row in rows:
            cells = [str(x) for x in row]
            md.append('| ' + ' | '.join(cells) + ' |')
            doc.append('<tr>' + ''.join('<td>' + html.escape(c) + '</td>' for c in cells) + '</tr>')
        md.append('')
        doc.append('</table>')

    def figure(cohort, filename):
        source = Path(sources[cohort]['path']).parent / filename
        target = out / 'figures' / (cohort + '-' + filename)
        shutil.copy2(source, target)
        raw = target.read_bytes()
        sources[str(target.relative_to(out))] = {'path': str(source), 'sha256': hashlib.sha256(raw).hexdigest()}
        md.extend([f'![{cohort}: {filename}](figures/{target.name})', ''])
        doc.append('<figure><img alt="' + html.escape(cohort + ': ' + filename) + '" src="data:image/png;base64,'
                   + base64.b64encode(raw).decode() + '"><figcaption>' + html.escape(cohort + ': ' + filename) + '</figcaption></figure>')

    heading('UpScaleDB：三个假设的探索与独立复测')
    text('首轮并行分区：H1 54 次、H2 60 次、H3 36 次；随后三个实验依次运行：H1 18 次、H2 60 次、H3 24 次。合计 252 次试验，全部通过各自正确性和分析检查，无失败或缺失。smoke 不计入试验数。')
    text('H1 使用 CPU0–7/node0，H2 使用 CPU16–17/node0，H3 使用 CPU32–39/node1，排除 SMT sibling；数据库内存本地绑定。分区不等于主机隔离。串行复测只消除了本批实验互相重叠，未证明整机独占。各 cell 三次重复，图中范围不是置信区间。首轮和复测不混合统计。')
    text('没有实现 parking、修改锁算法或覆盖原有 330 次 scalability 结果。此次只增加独立实验 harness/controller 与报告。')
    heading('H1：操作成本不同，公平分配优势是否存在？')
    text('H1 是真实数据库，8 个持续重发请求的 worker，三种 finder+inserter 比例，每次 5 秒。profile 中的 service 是 callback elapsed time，归属请求发起者，不是执行线程 CPU 时间。只计响应在截止前完成的完整 callback；不是截断到时间窗内的积分。Jain 指数基于服务时间，不基于操作计数。')
    rows = []
    for cohort in ('H1', 'H1-serial'):
        for cell in data[cohort]['decisions']['cost_asymmetry']:
            values = cell['insert_find_window_service_ratios']
            rows.append([cohort, cell['roles'], cell['variant'], f'{min(values):.3f}–{max(values):.3f}', cell['decision']])
    table(['批次', 'find+insert', 'profile 后端', 'insert/find 服务成本比范围', '描述性判断'], rows)
    table(['批次', 'find+insert', 'FC-PQ 对比', 'Jain 与角色份额误差同时改善？'],
          [[cohort, c['roles'], c['comparator'], c['decision']] for cohort in ('H1', 'H1-serial')
           for c in data[cohort]['decisions']['fc_pq_allocation']])
    text('判读：yes 要求三次配对均提高全 worker 服务 Jain 且减少 finder 份额相对 worker 数/8 的偏差；no 要求两项均未改善；其余为 inconclusive。这是描述性筛选，不是显著性检验或严格份额权利证明。没有 pending-request 时间线。FC-PQ 相对 FC 的改善与相对 USCL 的结果必须分开。')
    text('下面仅列串行 4+4 的未插桩 primary 均值，服务公平性数字不能代替这些吞吐/CPU/延迟证据。每个值来自三个进程。')
    rows = []
    trials = data['H1-serial']['trials']
    for backend in ('fc', 'fc_pq', 'uscl'):
        cells = [r['metrics'] for r in trials if r['variant'] == backend]
        assert len(cells) == 3
        mean = lambda key: statistics.mean(c[key] for c in cells)
        rows.append([backend, f'{mean("find_ops_s"):.0f}', f'{mean("insert_ops_s"):.0f}',
                     f'{mean("timed_process_cpu_s"):.3f}', f'{mean("writer_max_no_progress_s") * 1000:.3f}'])
    table(['后端', 'find ops/s', 'insert ops/s', '工作阶段 CPU-s', '每次最差 writer gap 的均值 ms'], rows)
    for cohort in ('H1', 'H1-serial'):
        figure(cohort, 'profile-service.png')
        figure(cohort, 'primary-throughput.png')
    heading('H2：USCL 保留时间片能否阻塞已有竞争者？')
    text('这是合成机制诊断，不是数据库吞吐。A 已完成 release，之后一直在锁外等 B 返回；B 的到达目标距 release 为 0、50、500 或 5000 μs。每次先给两个等权线程 20 ms elapsed credit。直接观察版本从未改动的本地 USCL 源码编译，快照仅在无并发 API 调用时读取；另测冻结 Rust bridge，不能把直接快照冒充桥接内部观测。')
    text('并行判断：' + data['H2']['classification'] + '；串行判断：' + data['H2-serial']['classification'])
    table(['串行后端', '目标 release→request μs', '三个进程中位数的中位数 μs', '进程中位数范围 μs'],
          [[c['backend'], c['pause_us'], f'{c["median_of_process_medians_ns"]/1000:.3f}',
            f'{c["min_process_median_ns"]/1000:.3f}–{c["max_process_median_ns"]/1000:.3f}']
           for c in data['H2-serial']['groups']])
    text('支持范围：在 A 保留有效且尚未到期的时间片、B 未被 ban 的条件下，B 延迟追踪剩余 reservation；过期对照消失。每进程 12 个内部样本彼此相关，不能冒充 36 次独立重复。TSC 按进程校准；没有减去计时开销。仍不据此归因真实 DB 的全部延迟、宣称 futex 确实睡眠或提出 OS 调度因果结论。')
    for cohort in ('H2', 'H2-serial'):
        figure(cohort, 'process_delays.png')
        figure(cohort, 'remaining_vs_delay.png')
    heading('H3：间歇请求在真实数据库中的进展')
    text('4 find + 4 insert，4 秒，无 warmup。bursty worker 每完成 64 次操作后主动睡眠 5 ms；one_per_role 各有一个，half_per_role 各有两个，其余持续请求。串行只复测 continuous 与 one_per_role，不能称 half_per_role 已独立确认。响应时间不包括主动 sleep；completion gap 包括 sleep，不能当作 starvation。p99 是合并 histogram 的桶上界，包含 drain。')
    rows = []
    for cell in data['H3-serial']['summaries']:
        m = cell['metrics']
        p99 = m['request_p99_upper_bound_ns']['median']
        rows.append([cell['pattern'], cell['backend'], cell['role'] + '/' + cell['group'],
                     f'{m["throughput_per_worker_ops_s"]["median"]:.1f}',
                     'NA' if p99 is None else f'{p99 / 1e6:.6f}'])
    table(['串行请求模式', '后端', '角色/组', 'ops/s/worker 中位数', '组 p99 桶上界的中位数 ms'], rows)
    text('该负载检验间歇请求表现，不直接测量“有 backlog 却无人执行”的时间。USCL 的慢进展是否全部由 H2 的 reservation 导致，尚不能定量归因；必须比较 continuous 基线。FC 与 FC-PQ 的 bursty 进展若接近，也不能把相对 USCL 的优势独归于公平调度。')
    for cohort in ('H3', 'H3-serial'):
        for filename in ('throughput_per_worker_ops_s.png', 'request_p99_upper_bound_ns.png', 'request_max_ns.png'):
            figure(cohort, filename)
    heading('证据与复现')
    text('所有原始 JSON、运行命令、随机顺序、CPU/NUMA placement、编译/冻结库和源文件哈希，以及 smoke 证据，保留在输入根目录的六个独立子目录中。原始目录被 Git 忽略，提交报告不等于备份原始数据。报告 HTML 内嵌全部 14 张 PNG；provenance.json 记录输入 summary 与图像 SHA-256。')
    text('复现分析：分别调用 hypothesis_roles.py、hypothesis_reservation.py、hypothesis_bursts.py 的 --analyze-only --output-root 参数；原分析目录只读，H2 可指定新的 --analysis-root，H3 重分析须使用保留原数据的新目录。报告命令：python3 integration/upscaledb/hypothesis_report.py --input-root .worktree/upscaledb-hypotheses --output-dir 新报告目录。不得通过删除旧结果来重跑。')
    (out / 'report.md').write_text('\n'.join(md))
    (out / 'report.html').write_text('<!doctype html><html lang="zh"><meta charset="utf-8"><title>UpScaleDB hypotheses</title><style>body{max-width:1150px;margin:2em auto;font:16px/1.6 sans-serif;padding:1em}img{max-width:100%}table{border-collapse:collapse}td,th{border:1px solid #aaa;padding:.4em}figure{margin:2em 0}</style><body>' + '\n'.join(doc) + '</body></html>')
    (out / 'provenance.json').write_text(json.dumps({'input_root': str(root), 'sources': sources, 'generator_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}, ensure_ascii=False, indent=2) + '\n')
    print(f'Wrote {out}: 252 validated trials, 14 figures, Markdown and portable HTML')


if __name__ == '__main__':
    main()
