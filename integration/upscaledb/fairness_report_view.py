"""Portable Chinese presentation of the campaign's existing measured results."""
import base64
import html
import json
import re


def esc(value):
    return html.escape(str(value))


def render_report(out, data, figures, text):
    def svg(title, body, height):
        return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 900 {height}" role="img" '
                f'aria-label="{esc(title)}"><title>{esc(title)}</title><g font-family="system-ui,sans-serif">{body}</g></svg>')

    def ratio_chart():
        body = ''
        colors = ('#168575', '#d17a36')
        for j, durability in enumerate(('immediate', 'none')):
            row = next(r for r in data['redb_pairs'] if r['cohort'] == 'half1_half64' and r['durability'] == durability)
            y = 60 + j * 130
            body += f'<text x="22" y="{y}" font-size="18" font-weight="700">{durability.title()}</text>'
            for i, (key, label) in enumerate((('short_tx_s', '小事务完成速度'), ('throughput_records_s', '总记录写入速度'))):
                s = row[key]; cy = y + 30 + i * 43
                x = lambda v: 260 + v * 270
                body += f'<text x="22" y="{cy+5}" font-size="16">{label}</text>'
                body += f'<line x1="{x(s["min"])}" x2="{x(s["max"])}" y1="{cy}" y2="{cy}" stroke="{colors[i]}" stroke-width="5"/>'
                body += f'<circle cx="{x(s["median"])}" cy="{cy}" r="7" fill="{colors[i]}"/>'
                body += f'<text x="785" y="{cy+5}" font-size="17" fill="{colors[i]}">{s["median"]:.3f}×</text>'
        body += '<line x1="530" x2="530" y1="30" y2="285" stroke="#64748b" stroke-dasharray="5 5"/>'
        for tick in (0, .5, 1, 1.5, 2):
            body += f'<text x="{260+270*tick}" y="320" text-anchor="middle" fill="#526175" font-size="14">{tick:g}×</text>'
        return svg('FC-PQ相对FC：小事务更快，总记录吞吐下降。点为配对中位数，线为三次范围。', body, 345)

    def fairness_chart():
        body = ''
        for j, mix in enumerate(('reads', 'single', 'batch')):
            y = 48 + j * 156
            label = {'reads': '纯读：原本就公平', 'single': '单条写入：有改善', 'batch': '八条批量：改善有限'}[mix]
            body += f'<text x="20" y="{y}" font-size="18" font-weight="700">{label}</text>'
            for i, (backend, color) in enumerate((('fc','#697a90'), ('fc_pq','#168575'), ('mcs','#4588bf'), ('uscl','#a06dba'))):
                row = next(r for r in data['heterogeneous'] if (r['mix'],r['placement'],r['backend']) == (mix,'shared',backend))
                v = row['service_jain_by_domain'][0]['median']; cy = y + 15 + i * 28
                body += f'<text x="32" y="{cy+17}" font-size="14">{backend.upper().replace("_","-")}</text><rect x="145" y="{cy}" width="{v*605}" height="18" rx="4" fill="{color}"/><text x="{155+v*605}" y="{cy+15}" font-size="14">{v:.3f}</text>'
        body += '<text x="145" y="514" font-size="14" fill="#526175">0</text><text x="750" y="514" font-size="14" text-anchor="end" fill="#526175">1 · 服务分配完全均等</text>'
        return svg('共享环境服务公平性：FC、FC-PQ、MCS与USCL的profile Jain中位数。', body, 540)

    def share_chart():
        body = ''
        for j, durability in enumerate(('immediate', 'none')):
            y = 42 + j * 115
            body += f'<text x="20" y="{y}" font-size="18" font-weight="700">{durability.title()}</text>'
            for i, backend in enumerate(('fc','fc_pq')):
                row = next(r for r in data['redb'] if (r['cohort'],r['durability'],r['backend']) == ('half1_half64',durability,backend))
                v = row['first_four_service_share']['median']; cy = y + 14 + i*37
                body += f'<text x="24" y="{cy+20}" font-size="16">{backend.upper().replace("_","-")}</text><rect x="145" y="{cy}" width="600" height="26" rx="4" fill="#e9c29f"/><rect x="145" y="{cy}" width="{600*v}" height="26" rx="4" fill="#168575"/><text x="770" y="{cy+20}" font-size="16">{v:.1%}</text>'
        body += '<line x1="445" x2="445" y1="45" y2="246" stroke="#334155" stroke-width="2" stroke-dasharray="5 4"/><text x="445" y="278" text-anchor="middle" font-size="14">50%：两组人数相同的等份目标</text>'
        return svg('四个小事务客户端的服务份额；绿色为小事务组，浅橙为大事务组，虚线为50%目标。',body,305)

    new_charts = [('redb-tradeoff',ratio_chart()), ('upscaledb-fairness',fairness_chart()), ('redb-service-share',share_chart())]
    for name, content in new_charts:
        (out / (name+'.svg')).write_text(content)
    def picture(name, caption):
        encoded = base64.b64encode((out/(name+'.png')).read_bytes()).decode()
        return f'<figure><div class="plot" tabindex="0" role="region" aria-label="可横向滚动的图表"><img loading="lazy" src="data:image/png;base64,{encoded}" alt="{esc(caption)}"></div><figcaption>{esc(caption)}</figcaption></figure>'
    def chart(index, caption):
        return f'<figure><div class="plot" tabindex="0" role="region" aria-label="可横向滚动的图表">{new_charts[index][1]}</div><figcaption>{esc(caption)}</figcaption></figure>'

    style = '''
:root{--ink:#172b43;--muted:#526175;--teal:#168575;--line:#dce5ed;--paper:#fff;--bg:#f1f5f8}*{box-sizing:border-box}html{scroll-behavior:smooth;scroll-padding-top:24px}body{margin:0;background:var(--bg);color:var(--ink);font:17px/1.85 system-ui,-apple-system,"Noto Sans CJK SC",sans-serif}main{max-width:1120px;margin:auto;padding:42px 28px 80px}header{background:#152c44;color:white;padding:42px;border-radius:20px}header p{color:#d0dee9;max-width:780px}h1{font-size:clamp(28px,4vw,44px);line-height:1.35;margin:12px 0}h2{font-size:27px;line-height:1.5;margin:0 0 16px}h3{font-size:20px}.eyebrow{font-size:13px;letter-spacing:2px;font-weight:700;color:#9edbd0}nav{display:flex;gap:12px;flex-wrap:wrap;margin:24px 0}a{color:#116a65;text-underline-offset:4px}nav a{background:white;border:1px solid var(--line);border-radius:20px;padding:5px 15px;text-decoration:none;font-size:14px}section{background:white;padding:32px;border:1px solid var(--line);border-radius:16px;margin:24px 0}.lead{font-size:20px;font-weight:600}.grid{display:grid;grid-template-columns:repeat(3,1fr);gap:16px;margin:24px 0}.card{background:white;border:1px solid var(--line);padding:22px;border-radius:14px}.card strong{display:block;font-size:28px;color:var(--teal)}.card span{font-size:14px;color:var(--muted)}.note,.warning{border-left:4px solid var(--teal);background:#eef8f5;padding:15px 20px;margin:20px 0}.warning{border-color:#cf813b;background:#fff6ec}.muted,figcaption{color:var(--muted);font-size:14px}figure{margin:24px 0;padding:16px;border:1px solid var(--line);border-radius:12px;background:#fff}figure svg,figure img{display:block;width:100%;height:auto}figcaption{margin-top:12px;line-height:1.7}details{margin:16px 0;border:1px solid var(--line);border-radius:10px;padding:15px}summary{cursor:pointer;font-weight:650}details p{margin:14px 0}.scroll{overflow:auto;max-width:100%}table{border-collapse:collapse;font-size:13px;white-space:nowrap;margin:14px 0;width:100%}th,td{padding:10px 14px;border-bottom:1px solid var(--line);text-align:left}th{background:#edf3f7;color:#304b65}tbody tr:nth-child(even){background:#f8fafc}pre{white-space:pre-wrap;overflow-wrap:anywhere;font-size:13px;background:#f5f7fa;padding:16px}p{overflow-wrap:anywhere}.flow{display:flex;align-items:stretch;gap:12px}.flow>div{flex:1;border:1px solid var(--line);padding:20px;border-radius:12px}.flow b{display:block;color:var(--teal)}.arrow{align-self:center;color:var(--muted)}.badge{font-size:12px;border:1px solid #bdcbd6;border-radius:5px;padding:2px 7px;margin-right:8px}.section-no{color:var(--teal);font-size:14px;letter-spacing:2px;font-weight:bold}footer{font-size:13px;color:var(--muted);padding:12px}@media(max-width:650px){main{padding:18px 12px}header,section{padding:22px 18px}.grid{grid-template-columns:1fr}.flow{flex-direction:column}.arrow{transform:rotate(90deg)}h2{font-size:23px}figure{padding:6px}body{font-size:16px}}@media print{body{background:white}main{padding:0}section,figure{break-inside:avoid}nav{display:none}header{color:var(--ink);background:#eef3f6}header p{color:var(--muted)}}
@media(max-width:650px){.plot{overflow-x:auto}figure svg,figure img{min-width:680px}figure::before{content:"左右滑动查看完整图表";display:block;color:var(--muted);font-size:12px;margin:6px 0 12px}}
'''
    parts = [f'<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>公平委托锁：收益、代价与边界</title><style>{style}</style></head><body><main>',
        '<header><div class="eyebrow">实验解读 · FC / FC-PQ / MCS / USCL</div><h1>更公平，值得付出多少代价？</h1><p class="lead">FC-PQ能给小事务更多服务机会，但不是所有负载的最优解。</p><p>从真实数据库的大小事务出发，看清公平性收益、吞吐与CPU代价，再看优势在哪里消失。</p></header>',
        '<nav aria-label="报告目录"><a href="#takeaway">一分钟结论</a><a href="#redb">关键收益</a><a href="#cost">付出的代价</a><a href="#counterexample">反例</a><a href="#contention">竞争边界</a><a href="#method">如何读这些数据</a><a href="#appendix">完整证据</a></nav>',
        '<div class="grid"><div class="card"><strong>909 次</strong><span>最终正式运行全部通过；不含保留的早期失败队列</span></div><div class="card"><strong>3 条实验线</strong><span>redb事务 / UpScaleDB异质客户端 / 高竞争矩阵</span></div><div class="card"><strong>2 秒窗口</strong><span>每次短窗口探索；3或5次重复，不作显著性声明</span></div></div>',
        '<section id="takeaway"><div class="section-no">01 / 先看结论</div><h2>不是免费提速，而是有条件地重新分配服务</h2><div class="flow"><div><b>值得：大小事务竞争</b>短事务获得更多服务，完成速度提高；总写入记录数会下降。</div><div><b>不一定：批量读写混合</b>UpScaleDB八条批量写入时，公平改善很有限，USCL更公平。</div><div><b>不值得：本来就公平</b>纯读或分离环境缺少可修复的公平问题，额外调度成本更明显。</div></div><p class="muted">这里的“公平”指串行服务时间的分配，不是每个人完成一样多的请求，也不是所有人的等待时间相同。</p></section>',
        '<section id="redb"><div class="section-no">02 / 最清楚的收益</div><h2>redb：让小事务不再只分到少量服务</h2><p>8个客户端竞争同一个写事务入口：4个每事务写1条记录，4个写64条。对比FC与FC-PQ，数据库做的仍是真实写入与commit。</p>',
        '<div class="flow"><div><b>普通轮流 ≠ 服务等份</b>大事务一次执行更久，即便轮流次数相同，也可能占用更多串行服务。</div><span class="arrow">→</span><div><b>按已用服务选择</b>FC-PQ尝试优先服务累计使用较少的请求者。</div><span class="arrow">→</span><div><b>完成混合改变</b>更多小事务完成，但大批量写入占比下降，总records/s可能降低。</div></div><p class="muted">上图是机制示意，不是实测执行轨迹，也不承诺任意同步到达下都能达到等份。</p>',
        chart(0,'实测 · primary。FC=1：右侧更快，左侧更慢。点为三次逐次配对比值的中位数，横线为最小—最大范围，不是置信区间。'),
        chart(2,'实测 · 独立profile。绿色为四个小事务客户端的服务份额中位数，浅橙为大事务组的剩余份额。机会更接近均等，但仍未达到50%目标。'),
        '<div class="note">读图结论：小事务完成速度约提高50%–62%，总记录吞吐的配对中位数约下降17%。这是服务分配的折中，不能把全部吞吐差值算作优先队列开销。</div></section>',
        '<section id="cost"><div class="section-no">03 / 收益背后的账单</div><h2>更多完成机会，不保证更好的p99</h2><p>redb 1/64混合下，小事务p99桶上界中位数：Immediate从FC的2.097ms升至FC-PQ的8.389ms；None从1.049ms升至4.194ms。更多小事务完成，不代表最慢的那些请求更快返回。</p><div class="warning">CPU也不便宜：FC、FC-PQ、MCS每两秒约消耗15–16个CPU秒；Native/Mutex通常约1.2–2.6秒。CPU秒可累加多个核，不能当成运行墙钟时间。</div>',
        picture('redb-cpu-cost','实测 · primary。纵轴为进程CPU秒 / 两秒测量窗口的秒数，近似展示占用多少核的CPU资源，包含drain；点为每次运行。Immediate与None分开比较。'),
        '<p>Native/Mutex虽然省CPU，但各自18个primary配置都出现过至少一个客户端在整个两秒窗口内没有完成事务。FC和MCS没有250ms零进展窗口；FC-PQ仍有一次运行出现一个这样的窗口。不能只看完成请求的p99，也不能称FC-PQ“无停顿”。</p></section>',
        '<section id="counterexample"><div class="section-no">04 / 不能略过的反例</div><h2>UpScaleDB：异质性更大，并不保证FC-PQ更有优势</h2><p>single与batch都包含4个点查客户端、4个写客户端；batch一次调度提交8次insert。它不是数据库事务，失败可能部分提交。</p>',
        chart(1,'实测 · 独立profile，共享环境。Jain越接近1，客户端服务时间越均等；条形从0起画。每项为三次中位数，完整范围见附录。纯读有8个读客户端。'),
        '<div class="warning">八条batch：FC-PQ的Jain仅0.628，与MCS的0.629接近，低于USCL的0.908；读客户端合计服务份额仅11.5%，距离50%很远。FC-PQ没有改善读尾延迟。</div><p>纯读时FC与FC-PQ本来都接近完全公平，但FC-PQ/FC的primary吞吐配对中位数仅0.397。把读写分进两个独立环境后，也不能再按八个客户端的全局服务份额评价公平：那是两个各有四人的公平域。</p><details><summary>为什么理想调度没有直接变成理想服务份额？</summary><p><span class="badge">解释候选 · 未验证因果</span>同步客户端持续运行，不等于它的请求在每次选择时都可见、可选择。源码存在combine入口吸收公告、完成节点暂存和combiner延迟返回等路径。本次没有admission/selection时间线，不能确定哪个因素贡献最大；源码保留累计usage，不是每次入队清零。</p></details></section>',
        '<section id="contention"><div class="section-no">05 / 性能优势的边界</div><h2>请求集中后，相对MCS更快；系统总吞吐却更低</h2>',
        picture('contention-frontier','实测 · primary。每条线连接配对比值中位数，散点为五次观测；虚线1为持平。uniform=均匀访问32个独立环境，hot90=90%热表目标，hot100=全部访问一个热表。左图与MCS比，右图与FC比。'),
        '<p>32 workers：均匀分布时FC-PQ/MCS为0.743倍，全热点时变为1.105倍。然而FC-PQ的绝对吞吐从约1045万降至102万op/s。热点只改善了相对位置，不是提升总性能的办法。</p><p class="muted">[INFERENCE] combining摊销与数据复用可能改善，但热点还改变B树增长及工作集；没有cache迁移或每pass实际完成批次数据，不能声称已经分解出局部性贡献。</p></section>',
        '<section id="method"><div class="section-no">06 / 阅读规则</div><h2>先分清测量，再谈结论</h2><div class="grid"><div class="card"><b>primary：看主要性能</b><p class="muted">未加入服务计时插桩的主运行，用于主要吞吐与CPU比较。</p></div><div class="card"><b>profile：看服务分配</b><p class="muted">独立计时运行，服务墙钟包含执行和可能的I/O，不是CPU时间。</p></div><div class="card"><b>范围：看重复差异</b><p class="muted">最小—最大是实测范围，不是置信区间；三次或五次不构成显著性证明。</p></div></div><div class="warning">插桩影响不可忽略。UpScaleDB batch/shared中，FC-PQ的profile/primary请求吞吐比为0.732–1.867。不能将profile的公平性与primary的吞吐拼成同一次运行的精确折中点。</div><p>Immediate与None是不同持久化模式，不能混算。close/reopen校验通过不等于断电恢复测试。早期redb队列的7次容量失败保留；提高容量上限后完整重跑180次，没有只替换失败点。</p></section>',
        '<section id="appendix"><div class="section-no">07 / 完整证据</div><h2>深入阅读与全部数值</h2><p class="muted">正文聚焦读图；详细论证、原始范围、所有后端和复现信息保留在下面。所有图随HTML内嵌，可离线打开。</p>']
    parts.append('<details><summary>完整文字分析：机制、限制、反例与复现命令</summary>')
    for line in text.splitlines():
        if not line.strip(): continue
        tag = 'h3' if re.match(r'^\d+\. ',line) else 'p'
        parts.append(f'<{tag}>{esc(line)}</{tag}>')
    parts.append('</details><details><summary>补充图：完整redb公平性—吞吐散点与UpScaleDB逐客户端服务份额</summary>')
    parts.append(picture('redb-fairness-frontier','两个坐标均来自profile：横轴服务Jain，纵轴记录/s；各点为一次运行。不是primary性能与profile公平性的拼接。'))
    parts.append(picture('heterogeneous-service','共享环境的逐客户端服务份额；0–3为读者、4–7为写者。线为三次均值，散点为各次观测，虚线为1/8等份目标。'))
    parts.append('</details>')
    labels = {'workers':'物理核worker数','routing':'请求分布','mix':'负载','placement':'环境布局','backend':'后端','cohort':'事务大小组合','durability':'持久化模式','pq_vs_mcs':'FC-PQ / MCS','pq_vs_fc':'FC-PQ / FC','fc_pq_ops_s':'FC-PQ 操作/s','mcs_ops_s':'MCS 操作/s','reads_per_s':'读/s','inserted_records_per_s':'写记录/s','total_requests_per_s':'请求/s','cpu_seconds_per_second':'CPU秒/窗口秒','service_jain_by_domain':'各环境服务Jain（profile）','profile_reader_service_share':'读客户端服务份额（profile）','worst_reader_p95_ns':'最慢读客户端p95桶上界(ns)','max_requester_no_completion_gap_ns':'最长无完成间隔(ns)','zero_progress_client_windows':'零进展客户端窗口数','throughput_records_s':'记录/s','throughput_tx_s':'事务/s','short_tx_s':'小事务/s','process_cpu_seconds':'进程CPU秒','service_wall_jain':'服务Jain（profile）','first_four_service_share':'前四客户端服务份额（profile）','short_response_p99_ms_upper':'小事务p99桶上界(ms)','max_zero_progress_windows':'单客户端最大零进展窗口数'}
    def fmt(v):
        if isinstance(v,list): return '；'.join(f'环境{i}: {fmt(x)}' for i,x in enumerate(v))
        if isinstance(v,dict) and 'median' in v:
            if v['median'] is None: return '不适用'
            return f'{v["median"]:.4g} [{v["min"]:.4g}, {v["max"]:.4g}] · n={v["n"]}'
        return {'None':'不适用','shared':'共享','split':'分离','reads':'纯读','single':'单条写入','batch':'八条批量','all1':'全部1条','half1_half8':'1条 / 8条','half1_half64':'1条 / 64条'}.get(str(v),str(v))
    for key,title in [('contention','竞争边界：绝对吞吐与配对比值'),('heterogeneous_pairs','UpScaleDB：FC-PQ / FC配对比值'),('heterogeneous','UpScaleDB：七个后端的完整结果'),('redb_pairs','redb：FC-PQ / FC配对比值'),('redb','redb：五个后端的完整结果')]:
        rows=data[key]; columns=list(rows[0]); is_ratio=key.endswith('_pairs')
        parts.append(f'<details><summary>{title}</summary><p class="muted">中位数 [最小值, 最大值]；n为可用观测数。'+('所有性能列均为FC-PQ / FC比值，无单位。' if is_ratio else '服务公平性来自profile，其余主要性能来自primary。')+'</p><div class="scroll" tabindex="0" role="region" aria-label="可横向滚动的数据表"><table><thead><tr>')
        parts.extend(f'<th scope="col">{esc(labels.get(k,k))}{"（比值）" if is_ratio and isinstance(rows[0][k],dict) else ""}</th>' for k in columns)
        parts.append('</tr></thead><tbody>')
        for row in rows: parts.append('<tr>'+''.join(f'<td>{esc(fmt(row[k]))}</td>' for k in columns)+'</tr>')
        parts.append('</tbody></table></div></details>')
    parts.append('<details><summary>输入路径与SHA-256</summary><pre>'+esc(json.dumps(data['sources'],indent=2,ensure_ascii=False))+'</pre><p>原始数据位于Git-ignored .worktree。此便携报告不是原始数据备份；reduced.json保留配对观测与插桩扰动。</p></details></section><footer>最终909次运行通过 · 4张已有实测图 + 3张新增中文实测图 + 机制示意 · 不重跑实验，不改变数据或结论</footer></main></body></html>')
    (out/'report.html').write_text('\n'.join(parts))
