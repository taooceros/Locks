#!/usr/bin/env python3
"""Render the saved multi-table lock comparison without running measurements."""
import argparse
import base64
import hashlib
import html
import json
from pathlib import Path
import shutil
import statistics

from boundary_tables import figures as render_figures


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def reject(value):
    raise ValueError('nonfinite input: ' + value)


def spread(values, divisor=1):
    values = [v / divisor for v in values if v is not None]
    if not values:
        return 'NA'
    return f'{statistics.median(values):,.3f} [{min(values):,.3f}, {max(values):,.3f}]'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input-root', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    root, out = args.input_root.resolve(), args.output_dir.resolve()
    source = root / 'analysis/summary.json'
    summary = json.loads(source.read_text(), parse_constant=reject)
    manifest_path = root / 'manifest.json'
    manifest = json.loads(manifest_path.read_text(), parse_constant=reject)
    diagnostic_path = root.parent / 'other-locks-gate-diagnostics.json'
    diagnostics = json.loads(diagnostic_path.read_text(), parse_constant=reject)
    initial_root = root.parent / 'multitable-other-locks'
    initial_source = initial_root / 'analysis/summary.json'
    initial = json.loads(initial_source.read_text(), parse_constant=reject)
    rejected_source = root.parent / 'multitable-other-locks-cap32m/analysis/summary.json'
    rejected = json.loads(rejected_source.read_text(), parse_constant=reject)
    # Reviewed cohort narrative: require new review before publishing different data.
    for path, expected in (
        (source, '8df064b643f31feded55a8ca8009bace9f57edf849df24b1ab04b8dc310591f7'),
        (initial_source, 'e7f7c2ea0d9631db54ac7c6db3f451479786d004a7f3bfa5000c284ab91418ad'),
        (rejected_source, 'bf2826c23582811d851b8613bb86c2eb6c0766c7bedddb7b19d1904a01ac5886'),
        (diagnostic_path, 'd2fd72468e514bbe8549f1a275553e564fada6bd41ba531b3360cd30b3470519'),
    ):
        if sha(path) != expected:
            raise ValueError('Narrative needs review for changed cohort: ' + str(path))
    if out.exists():
        previous = json.loads((out / 'provenance.json').read_text(), parse_constant=reject)
        if previous['summary_sha256'] != sha(source):
            raise ValueError('Refusing to overwrite a report for a different cohort')
    out.mkdir(parents=True, exist_ok=True)
    (out / 'figures').mkdir(exist_ok=True)
    doc, text, figures = [], [], {}

    def heading(value):
        doc.append('<h2>' + html.escape(value) + '</h2>')
        text.extend(['', value, ''])

    def paragraph(value):
        doc.append('<p>' + html.escape(value) + '</p>')
        text.append(value)

    def table(headers, rows):
        doc.append('<div class="scroll"><table><tr>' + ''.join('<th>' + html.escape(str(v)) + '</th>' for v in headers) + '</tr>')
        text.append('\t'.join(headers))
        for row in rows:
            doc.append('<tr>' + ''.join('<td>' + html.escape(str(v)) + '</td>' for v in row) + '</tr>')
            text.append('\t'.join(map(str, row)))
        doc.append('</table></div>')

    heading('多表数据库：其它锁与同期基线')
    paragraph(f'预定 {summary["expected_trials"]} 次，成功 {summary["successful_trials"]} 次，失败 {summary["failed_trials"]} 次，缺失 {summary.get("missing_trials", 0)} 次。保留全部预定条件和失败证据；下表仅汇总通过校验的进程。')
    paragraph('1、8、32 张表，8 个固定物理核 worker，总初始记录32,768；每个worker交替执行find与唯一insert，比例50/50。coarse 是一个共享 environment/锁；split 是每表独立 environment/锁，不是擅自拆开同一 environment 的互斥保护。相同活线程先做只读注册 warmup；2秒测量窗口，三次独立重复。无人工周期 sleep。所有worker的操作类型序列相同，本实验适合检验多锁访问下的吞吐/延迟边界，不直接构成异质请求者服务公平性实验。')
    paragraph('首轮全部180次已执行：169次有效、11次因每worker100万次插入的资源保护上限而失败，均为split/32表的SpinLock、MCS、Ticket、CLH；失败进程的数据库、buffer、精确键集合、完整性与关闭检查正常。没有把截断运行计入吞吐比较，也没有只挑失败项补跑。保留首轮原始记录后，仅把插入保护上限提高到每worker400万次/总3200万次，32GiB地址空间上限及全部负载参数不变；重新通过120个门并整批重跑180次。以下数字只来自重跑批次，首轮不参与合并。')
    paragraph('容量修正后的中间批次也执行了180次，其中11个进程正常退出且harness failed=false，却被控制器残留的旧每worker100万阈值拒绝。这是本次工具修正的遗漏，不是锁错误。修正后全部180份原始结果回放校验通过，但不改写旧记录：最终再整批运行一次。以下仅使用最终完整流程批次，前两批的失败/拒绝证据分别保留，不参与汇总。')
    paragraph('Native、borrowed mutex、FC、FC-PQ、USCL 与新增锁在同一批次重跑。全局锁排除参与者的并行编译和计时；CPU56–63、node1绑定，不使用对应SMT sibling。无法保证排除无关宿主任务、缓存及功耗干扰。')
    paragraph('ShflLock 不纳入性能矩阵：现有 Rust 实现混用重叠 AtomicU8/AtomicU32 访问，并经共享 QNode 引用写非 UnsafeCell 字段。未修改算法或以另一个实现替代。CFL-local 是本地移植代理，不声称是已验证的论文原版。其它安全边界以原始分析报告为准。')
    paragraph('实现边界：CFL-local 不同句柄仍共享全局公平性/accounting 状态，不能把它当成完全独立的每锁公平策略。CLH 现有实现没有释放 raw queue node 的 Drop；此固定8-worker队列中每个已使用句柄至多泄漏1,152字节，32个句柄至多36,864字节，进程退出回收。本次不修改算法；这不是长期反复创建锁时无泄漏的保证。MCS/CLH 仅通过同一线程 lock→callback→unlock 使用，不测试或依赖 try_lock、锁内嵌套及跨线程 guard 转移。')
    paragraph('验证边界：本次8-worker矩阵的120个多表正确性/错误传播门全部通过；Rust主构建8项、profile/test-hooks构建11项、构建ABI测试11项通过。额外128-worker压力测试中，Ticket首次超过原120秒watchdog；将外部诊断时限设为600秒后，同一二进制约0.247秒完成，CLH约107.752秒完成。原正确性脚本没有一次性全绿，首次失败没有被删除。超时原因未确定；这些结果不保证超额订阅下的进展，也不是性能重复测量。未修改算法或原正确性脚本的watchdog。')
    paragraph('数值为三次重复的中位数 [最小, 最大]，不是置信区间。先逐次相加读写吞吐，再汇总总吞吐；不要相加两个操作类型各自的中位数。每操作类型的平均延迟单位us；p99为直方图桶上界且需要足够样本。NA不代表零延迟。CPU秒覆盖release到join，包含drain及TLS退出，不是纯锁或临界区开销。')
    rows = summary['measurements']
    backends = manifest.get('backends') or sorted({r['backend'] for r in rows})
    totals = {}
    for row in rows:
        key = (row['layout'], row['tables'], row['backend'], row['rep'])
        totals[key] = totals.get(key, 0) + row['throughput_ops_s']
    heading('本批次结论：优势与劣势边界')
    paragraph('共享一个environment时，FC-PQ在三个表数配置下均比Native和CFL-local有更高的吞吐，但没有稳定优于FC，也不是所有普通锁中最快。多表独立environment下，普通锁构成重要反例：split/8的MCS、Ticket、CLH，以及split/32的SpinLock、Ticket、CLH，在三次配对重复中均比FC-PQ快超过5%。这只是描述性筛选，不是显著性检验。SpinLock在split/8、MCS在split/32存在较大波动，不能按中位数给出稳定排名。')
    comparison_rows = []
    for count in (8, 32):
        for backend in ('spinlock', 'mcs', 'ticket', 'clh'):
            points = [totals[('split', count, backend, rep)] / totals[('split', count, 'fc_pq', rep)]
                      for rep in (1, 2, 3)]
            comparison_rows.append([count, backend, spread(points),
                                    '三次均>1.05' if all(point > 1.05 for point in points) else '混合/小幅（不等价）'])
    table(['独立environment数', '普通锁', '总吞吐比：普通锁/FC-PQ', '配对描述'], comparison_rows)
    paragraph('FC-PQ对FC的六个配置比较均未达到“三次一致超过5%”的优势筛选；对CFL-local，在共享environment和split/8下吞吐有一致优势，但split/32是混合结果。split/32总吞吐中位数：CLH8.740、SpinLock8.550、Ticket8.060、MCS7.151、FC6.207、FC-PQ5.704、Native5.670、CFL-local3.142 Mops/s。CFL-local范围3.115–6.397 Mops/s，不能用其中位数掩盖其波动。')
    paragraph('USCL的多锁退化仍然明显：split/8总吞吐中位数4,755.5 ops/s，split/32为9,303.5 ops/s；find平均延迟中位数分别约1.676ms和0.862ms。这不是FC-PQ独有的优势，普通锁和Native也避开了该量级退化。未做本批次锁占用追踪，不把全部差距归因为保留时间片导致的锁空闲。')
    paragraph('CPU不能略去：split/32中FC、FC-PQ、普通自旋/队列锁均使用约16进程CPU秒，Native约13.743秒；因此普通锁对FC-PQ的吞吐优势并非靠更高的该项CPU预算。CLH的find均值约0.603us，FC-PQ约1.050us。USCL虽使用约11.962CPU秒，却完成远少的操作；低绝对CPU不等于低单位工作成本。')
    paragraph('[INFERENCE] 多锁降低单锁竞争后，combining可摊薄的收益可能减少，而发布、扫描、优先队列和usage记账成本仍在，因此简单锁可能更合适。本次未记录批大小或拆分这些成本，不能把吞吐差异当成该因果解释的证明。下一步若要论证FC-PQ的价值，应保留这些劣势配置，并使用真正异质的请求者成本与服务份额指标，而不是只和USCL比吞吐。')
    for layout in ('coarse', 'split'):
        for count in (1, 8, 32):
            heading(f'{layout} / {count} 表')
            body = []
            for backend in backends:
                selected = [r for r in rows if (r['layout'], r['tables'], r['backend']) == (layout, count, backend)]
                finds = [r for r in selected if r['role'] == 'finds']
                inserts = [r for r in selected if r['role'] == 'inserts']
                rates = [v for (l, k, b, _), v in totals.items() if (l, k, b) == (layout, count, backend)]
                body.append([backend, f'{len(rates)}/3', spread(rates),
                             spread([r['process_cpu_ns'] for r in finds], 1e9),
                             spread([r['request_mean_ns'] for r in finds], 1e3),
                             spread([r['request_mean_ns'] for r in inserts], 1e3),
                             spread([r['request_p99_upper_ns'] for r in finds], 1e3)])
            table(['后端', '通过', '总ops/s', '进程CPU秒', 'find均值us', 'insert均值us', 'find p99上界us'], body)
    heading('所有配置图')
    plot_root = root / 'presentation'
    plot_root.mkdir(exist_ok=True)
    plot_metadata = render_figures(plot_root, rows, summary['failed_trials'], summary['missing_trials'],
                                   backends, manifest['excluded_backends'])
    for path in sorted(plot_root.glob('*.png')):
        relative = Path('figures') / path.name
        shutil.copy2(path, out / relative)
        figures[str(relative)] = {'source': str(path), 'sha256': sha(path)}
        doc.append('<figure><img alt="' + html.escape(path.name) + '" src="data:image/png;base64,' + base64.b64encode(path.read_bytes()).decode() + '"><figcaption>' + html.escape(path.name) + '</figcaption></figure>')
        text.append('Figure: ' + str(relative))
    heading('解释边界')
    paragraph('优先用本批同期控制做比较；旧批次只作背景，不与新锁拼成一次配对试验。K=1的两种布局在保护语义上相同，观察到的差异应保留为控制变动，不当成分片收益。split还改变environment缓存、分配和数据组织，因此不能把全部差异归因为锁粒度。吞吐、CPU和延迟分别判断，不选一个指标宣称全面胜出。')
    paragraph('本实验未直接测量锁占用或请求者服务份额。即使USCL多表下退化，也不能仅凭低吞吐证明全部时间是“有请求时锁空闲”；普通队列锁表现好也不等于证明FC-PQ的增量公平性。三个重复只能描述观察范围，不能给出稳定的小差距排名。')
    paragraph('绘图修正仅影响展示：原始symlog自动范围给非负数据添加了负半轴，并遗漏部分刻度标签。此报告按全部实际数据点设置非负范围与1/2/5刻度，不移除任何后端或观测值。原始分析与图片保留在analysis；新渲染保存在presentation。测量时控制器源码保留在sources/boundary_tables.py，绘图修订源码另行记录哈希。')
    original_report = root / 'analysis/report.txt'
    original = original_report.read_text()
    doc.append('<details><summary>原始分析：全部配对结果、失败与限制</summary><pre>' + html.escape(original) + '</pre></details>')
    text.extend(['', original])
    shutil.copy2(original_report, out / 'analysis-report.txt')
    shutil.copy2(diagnostic_path, out / 'gate-diagnostics.json')
    doc.append('<details><summary>额外压力测试：首轮超时说明与后续原始诊断</summary><pre>' + html.escape(json.dumps(diagnostics, ensure_ascii=False, indent=2)) + '</pre></details>')
    initial_failures = {key: initial[key] for key in ('expected_trials', 'successful_trials', 'failed_trials', 'missing_trials')}
    initial_failures['trials'] = [trial for trial in initial['trials'] if trial['status'] != 'ok']
    (out / 'initial-cap-failures.json').write_text(json.dumps(initial_failures, ensure_ascii=False, allow_nan=False, indent=2) + '\n')
    validation_rejections = {key: rejected[key] for key in ('expected_trials', 'successful_trials', 'failed_trials', 'missing_trials')}
    validation_rejections['trials'] = [trial for trial in rejected['trials'] if trial['status'] != 'ok']
    (out / 'intermediate-validation-rejections.json').write_text(json.dumps(validation_rejections, ensure_ascii=False, allow_nan=False, indent=2) + '\n')
    compact = {k: v for k, v in summary.items() if k not in ('trials', 'measurements', 'per_table_progress')}
    (out / 'summary.json').write_text(json.dumps(compact, ensure_ascii=False, allow_nan=False, indent=2) + '\n')
    provenance = {'input_root': str(root), 'summary_path': str(source), 'summary_sha256': sha(source),
                  'manifest_sha256': sha(manifest_path), 'analysis_report_sha256': sha(original_report),
                  'gate_diagnostics_sha256': sha(diagnostic_path),
                  'initial_summary_sha256': sha(initial_source),
                  'initial_manifest_sha256': sha(initial_root / 'manifest.json'),
                  'intermediate_summary_sha256': sha(rejected_source),
                  'generator_sha256': sha(Path(__file__)), 'figures': figures,
                  'plot_source_sha256': sha(Path(__file__).with_name('boundary_tables.py')),
                  'measurement_controller_sha256': sha(root / 'sources/boundary_tables.py'),
                  'plot_metadata': plot_metadata,
                  'expected_trials': summary['expected_trials'], 'successful_trials': summary['successful_trials'],
                  'failed_trials': summary['failed_trials'], 'missing_trials': summary.get('missing_trials', 0)}
    (out / 'provenance.json').write_text(json.dumps(provenance, ensure_ascii=False, indent=2) + '\n')
    (out / 'report.txt').write_text('\n'.join(text) + '\n')
    (out / 'report.html').write_text('<!doctype html><html lang="zh"><meta charset="utf-8"><title>Multi-table other locks</title><style>body{max-width:1400px;margin:2em auto;padding:1em;font:16px/1.6 sans-serif}img{max-width:100%}.scroll{overflow:auto}table{border-collapse:collapse}td,th{border:1px solid #aaa;padding:.4em}pre{white-space:pre-wrap;overflow-wrap:anywhere}</style><body>' + '\n'.join(doc) + '</body></html>')
    print(f'Published {summary["successful_trials"]}/{summary["expected_trials"]} trials and {len(figures)} figures: {out}')


if __name__ == '__main__':
    main()
