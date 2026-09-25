#!/usr/bin/env python3
"""Publish saved boundary evidence as portable HTML; never run measurements."""
import argparse
import base64
import hashlib
import html
import json
from pathlib import Path
import shutil
import statistics


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def reject(value):
    raise ValueError('nonfinite JSON: ' + value)


def load(path):
    return json.loads(path.read_text(), parse_constant=reject)


def number(value):
    return 'NA' if value is None else f'{value:.6g}' if isinstance(value, (int, float)) else str(value)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input-root', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    root, out = args.input_root.resolve(), args.output_dir.resolve()
    expected = {'tables': 90, 'database': 60, 'cost': 66, 'arrival': 90}
    # Narrative is a reviewed reduction of this cohort, not an automatic claim
    # about future reruns. Refuse replacement summaries with the same shape.
    reviewed = {
        'database': 'dac11294aa4da7968e7434329d07783d35c372f49fb5825ddf5c49155c748525',
        'cost': '84ac5f03f40a7274dceb17506cb85da1d18a7feafb7ed08999ea1b6e7fb48887',
        'arrival': 'b91efbbbde274f632b4038b3c46275cb35eb16b725a02589879f65e96cfe22f4',
        'tables': '29400d90799daf5fb7b61265cae79a0bef2fa04419bf38090255556defd0043d',
    }
    data, provenance = {}, {'sources': {}, 'figures': {}}
    for name, count in expected.items():
        path = root / name / 'analysis/summary.json'
        if name in reviewed and digest(path) != reviewed[name]:
            raise ValueError(f'{name}: narrative requires review for a new cohort')
        data[name] = load(path)
        if data[name]['expected_trials'] != count:
            raise ValueError(f'{name}: unexpected matrix size')
        provenance['sources'][name] = {'path': str(path), 'sha256': digest(path)}
    out.mkdir(parents=True, exist_ok=False)
    (out / 'figures').mkdir()
    (out / 'evidence').mkdir()
    doc, text = [], []

    def heading(title):
        doc.append('<h2>' + html.escape(title) + '</h2>')
        text.extend(['', title, ''])

    def paragraph(value):
        doc.append('<p>' + html.escape(value) + '</p>')
        text.append(value)

    def table(headers, rows):
        doc.append('<div class="scroll"><table><thead><tr>' + ''.join('<th>' + html.escape(str(v)) + '</th>' for v in headers) + '</tr></thead><tbody>')
        text.append('\t'.join(headers))
        for row in rows:
            doc.append('<tr>' + ''.join('<td>' + html.escape(str(v)) + '</td>' for v in row) + '</tr>')
            text.append('\t'.join(map(str, row)))
        doc.append('</tbody></table></div>')

    heading('FC / FC-PQ / USCL：优势、代价与多表边界')
    paragraph('四组独立实验；冻结原始锁与库，不引入 parking 或新的调度算法。准备工作可并行，计时阶段通过全局互斥锁串行执行，dashboard 停止。CPU 分区避免参与者重叠，但不能排除无关宿主任务、共享缓存、内存或功耗干扰。')
    table(['实验', '预定进程数', '通过', '失败'], [[name, count, data[name]['successful_trials'], data[name]['failed_trials']] for name, count in expected.items()])
    paragraph('每个条件只有三次独立重复，图中是原始点及观察范围，不是置信区间。预声明的逐指标筛选：三次配对比值全部 >1.05 或全部 <0.95 才记为一致方向；其余是混合/变化较小，不是等价或统计显著性。保留全部配置，不按结果挑选胜者。')
    paragraph('数据库和多表实验是受限的真实内存 UpScaleDB 操作；成本实验是合成临界区；Poisson 到达流也是合成负载，不是生产 trace。不同 CPU 分区的绝对吞吐不作跨实验排名。')

    descriptions = {
        'database': ('单表：工作域与竞争强度',
            '初始读取域为 1,000 或 1,000,000 条；1 或 8 个 worker。单 worker 交替读写，8 worker 是 4 find + 4 insert，不能把两者当成完全相同的角色模型。插入持续增长数据库，因此初始读取域不等于固定的最终工作集。',
            '单线程下 FC-PQ 相对 FC 的总吞吐配对比值为约 0.835–0.838（小读取域）及 0.888–0.896（大读取域），表现出调度路径的额外成本。8 worker 下 FC 和 FC-PQ 均比 Native 快，但 FC-PQ 不稳定优于 FC；大读取域三次都低于 FC。该实验没有独立服务公平性计时，也不能证明缓存一致性是收益原因。'),
        'cost': ('受控成本：公平性不等于操作数',
            '固定共享状态；便宜/昂贵请求执行 64/1024 次依赖整数和查表迭代。1/8 个等成本 worker，或 4+4、7+1 成本不对称。48 次主测量为 2 秒，18 次独立 profiling 为 1 秒；无 warmup。合成迭代数不是实测服务时间。',
            'FC-PQ 在单线程控制中付出吞吐和 CPU/操作代价；在 7+1 中更多便宜操作推高操作吞吐，却降低相对 FC 的迭代工作量吞吐。独立 profiling 中 FC-PQ 的 Jain 服务指数相对 FC 改善，但 4+4 仍只有约 0.628（FC 约 0.585，USCL 约 0.946）；7+1 约为 0.893（FC 约 0.307，USCL 约 0.964）。不能把它写成普遍达到使用公平。Profiling 改变了测量路径和窗口，不能将其公平性数值直接绑定到未插桩主测量。'),
        'arrival': ('统一到达流：相同需求下的响应与资源',
            '8 个稳定 requester，95/5 与 50/50 find/insert，预生成 Poisson 到达时间，在所有后端复用。0.3/0.7/1.1 是同一个冻结 Native 基准的倍数，不是各后端利用率；95/5 基准因两百万请求上限从约 927,792/s 下调至 826,446/s。2 秒窗口，最多 5 秒额外提交/drain，统计全部到达分母与未完成请求。',
            '低请求率下吞吐主要受 offered load 限制，接近不能证明锁等价。两种读写比例的低负载中，USCL 计划到达至完成 p99 桶上界中位数约 16.777 ms，FC/FC-PQ 约 0.131 ms。95/5 最高请求率下 USCL 窗口完成比例中位数约 74.7%，FC/FC-PQ 接近全部完成；但 USCL 窗口 CPU 更低。FC-PQ 并非优于 FC：例如 95/5 中档，吞吐相近而三次 CPU 均更高；最高档 FC-PQ 的调度尾延迟也更高。这里只观察积压和延迟，没有测量锁的占用状态，不能把所有差异归因为 reservation。'),
        'tables': ('多表：共享 environment 与安全独立环境',
            '1、8、32 张表，8 个 worker，总初始记录 32,768，50/50 读/唯一插入，确定性均匀表选择。coarse 共享一个 environment 及其原始锁；split 每表独立 environment/锁。没有对同一 environment 的表擅自使用独立锁，也没有人为的周期 sleep。同一批活着的 worker 先对每张表做八轮只读 warmup，完成注册后再同时开始；所有线程及 TLS 析构结束后才校验与销毁。',
            '多表独立环境下 USCL 出现数量级退化：8 表总吞吐中位数由 coarse 的 597,829 降至 split 的 4,617 ops/s；32 表由 590,584.5 降至 9,613 ops/s。8 表 split 的 Native / FC / FC-PQ 为约 332 / 382 / 345 万 ops/s。USCL 的 find 平均延迟（三次均值的中位数）约 1.728 ms，其他后端约 1.7–2.0 us。这支持用户提出的多锁访问场景值得重点研究，而非证明 FC-PQ 独有优势。多环境还改变缓存、分配和环境状态组织；未直接记录锁占用，因此不能将全部降幅定量归因于 reservation。')}

    for name in expected:
        title, design, interpretation = descriptions[name]
        heading(title)
        paragraph(design)
        paragraph(interpretation)
        summary = data[name]
        if name == 'tables':
            paragraph('重要限制：K=1 的 coarse/split 语义相同，但 FC/FC-PQ 仍有很大的重复间及布局标记间差异；其原因未确定，不能解释为单表拆分收益，也不能据此精确估计 FC 与 FC-PQ 的小幅优劣。USCL 的 K=8/32 退化在三次重复均出现，观察范围与其他后端相隔数量级。下表列出全部控制，不删除波动结果。')
            paragraph('表中单位为千 ops/s，先逐次相加 find+insert，再取三次中位数 [最小, 最大]。USCL split K=8/32 每类请求样本不足预定的 10,000 条 p99 门槛，图上缺失，不按零延迟处理；平均延迟仍可报告。进程 CPU/操作包含 drain 和 TLS teardown，不是临界区自身开销。')
            backends = ('native', 'bridge_mutex', 'fc', 'fc_pq', 'uscl')
            totals = {}
            for row in summary['measurements']:
                key = (row['layout'], row['tables'], row['backend'], row['rep'])
                totals[key] = totals.get(key, 0) + row['throughput_ops_s']
            compact_rows = []
            for layout in ('coarse', 'split'):
                for count in (1, 8, 32):
                    values = [layout, count]
                    for backend in backends:
                        points = [v / 1000 for (l, k, b, _), v in totals.items()
                                  if (l, k, b) == (layout, count, backend)]
                        values.append('NA' if len(points) != 3 else
                                      f'{statistics.median(points):,.3f} [{min(points):,.3f}, {max(points):,.3f}]')
                    compact_rows.append(values)
            table(['布局', '表数', *backends], compact_rows)
        if summary.get('integrity_errors') or summary['failed_trials']:
            paragraph('警告：该组存在失败或完整性问题；查看下方原始摘要，不将失败进程作为有效性能样本。')
        for path in sorted((root / name / 'analysis').glob('*.png')):
            relative = Path('figures') / (name + '-' + path.name)
            shutil.copy2(path, out / relative)
            encoded = base64.b64encode(path.read_bytes()).decode()
            caption = name + ': ' + path.name
            doc.append('<figure><img alt="' + html.escape(caption) + '" src="data:image/png;base64,' + encoded + '"><figcaption>' + html.escape(caption) + '</figcaption></figure>')
            text.append('Figure: ' + str(relative))
            provenance['figures'][str(relative)] = {'source': str(path), 'sha256': digest(path)}
        source_report = root / name / 'analysis/report.txt'
        report = source_report.read_text()
        copied = out / 'evidence' / (name + '-report.txt')
        shutil.copy2(source_report, copied)
        provenance['sources'][name]['report_sha256'] = digest(source_report)
        doc.append('<details><summary>全部分组、配对方向与限制（原始分析报告）</summary><pre>' + html.escape(report) + '</pre></details>')
        text.extend(['', report])
        compact = {k: v for k, v in summary.items() if k not in ('trials', 'raw_record_hashes')}
        (out / 'evidence' / (name + '-summary.json')).write_text(json.dumps(compact, ensure_ascii=False, allow_nan=False, indent=2) + '\n')

    heading('该保留哪些实验，能够支持什么')
    paragraph('成本不对称实验保留为机制与负面结果：区分服务份额、操作数、实际工作量和长请求代价，不用于证明应用吞吐。单表 1/8 worker 保留为无竞争成本与 contention 收益边界。统一到达流用于相同需求下的完成率、响应时间和 CPU 比较，不因吞吐接近就忽略延迟。多表布局用于检验用户提出的跨表访问场景；必须同时呈现 coarse/split 及单表控制。')
    paragraph('结论应分成 delegation 收益、FC-PQ 相对 FC 的增量公平性与代价、USCL 的延迟/CPU/服务公平性取舍。当前证据不能支持 FC-PQ 全面优于 FC/USCL，不能证明严格 work conservation、生产真实性或普遍可扩展性。后续算法改动不属于本批冻结基线结果。')
    heading('证据与复现')
    paragraph('输入根目录：' + str(root) + '。四个子目录保存源文件快照、编译和库哈希、资源分配、完整命令、smoke、随机顺序、原始进程记录和分析 CSV/JSON。原始文件不覆盖；本报告是归约，不替代原始数据备份。')
    paragraph('分析入口为 boundary_database.py、boundary_cost.py、boundary_arrival.py、boundary_tables.py；分别在原 worktree 使用 --analyze-only --output-root。再次分析应按对应 CLI 指定新目录或保留原始结果的新副本，不删除旧证据。报告入口：boundary_report.py --input-root 原始根目录 --output-dir 新目录。HTML 内嵌全部 PNG，可离线打开。')
    provenance.update({'input_root': str(root), 'generator_sha256': digest(Path(__file__)), 'expected_trials': sum(expected.values()), 'successful_trials': sum(d['successful_trials'] for d in data.values()), 'failed_trials': sum(d['failed_trials'] for d in data.values())})
    page = '<!doctype html><html lang="zh"><meta charset="utf-8"><title>UpScaleDB boundary evidence</title><style>body{max-width:1200px;margin:2em auto;padding:1em;font:16px/1.6 sans-serif}img{max-width:100%}table{border-collapse:collapse}td,th{border:1px solid #aaa;padding:.4em}.scroll{overflow:auto}pre{white-space:pre-wrap;overflow-wrap:anywhere}figure{margin:2em 0}</style><body>' + '\n'.join(doc) + '</body></html>'
    (out / 'report.html').write_text(page)
    (out / 'report.txt').write_text('\n'.join(text) + '\n')
    (out / 'provenance.json').write_text(json.dumps(provenance, ensure_ascii=False, allow_nan=False, indent=2) + '\n')
    print(f'Published {provenance["successful_trials"]}/{provenance["expected_trials"]} trials, {len(provenance["figures"])} figures: {out}')


if __name__ == '__main__':
    main()
