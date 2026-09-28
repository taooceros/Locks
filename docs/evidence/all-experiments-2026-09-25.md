Copied 2026-09-26 verbatim from the experiment ledger; only the relative links were rewritten as plain `branch@commit:path` text so nothing points outside this worktree.
Source: `experiment/upscaledb-fc-pq-integration@dcbfacb:docs/reports/all-experiments-2026-09-25.md`.
Any `.worktree/` path below is a git-ignored raw artifact on the machine that ran the experiment; it is not in any branch.

# Locks 全部实验总结：FC-PQ 的价值、代价与适用边界

日期：2026-09-25。范围：当前各 worktree 中可追溯的历史微基准、实现与数学验证，以及已完成的 UpScaleDB / redb 实验。本文只汇总既有证据，没有重跑计时实验或修改锁算法；复制到多个 worktree 的报告只计一次。

**建议先读研究发现图文版：HTML（`experiment/upscaledb-fc-pq-integration@dcbfacb:docs/reports/research-findings-2026-09-25/report.html`） · 14 页 PDF（`experiment/upscaledb-fc-pq-integration@dcbfacb:docs/reports/research-findings-2026-09-25/report.pdf`）。** 八张新图按“问题→发现→解释→反例”组织，另附四张支持图；本文保留为完整实验账本。

## 1. 总结先行

**目前最有力的结论不是“FC-PQ 普遍更快”，而是：公平委托可以重新分配串行服务，使短请求客户端获得更多进展；收益取决于争用、请求异质性和实际可选择性，而且有吞吐、CPU 与返回尾延迟代价。**

主要发现：

1. **最适合作为应用主结果的是 redb 的 1/64 大小写事务混合。** 相比 FC，FC-PQ 将 profile 服务 Jain 从 0.891 提高到 0.992（Immediate），小事务 primary 吞吐提高到 1.500 倍；总记录吞吐降到 0.832 倍，小事务 p99 桶上界反而升高。这是明确的服务重新分配，而非无代价优化。
2. **delegation 的性能优势有条件。** 32-worker 高竞争实验中，FC-PQ/MCS 从均匀访问的 0.743 倍变为 hot90 的 1.135 倍、hot100 的 1.105 倍。但制造热点使 FC-PQ 自己的吞吐从约 1045 万降至 102 万 op/s；相对领先不等于系统变快。
3. **无明显服务失衡时，FC-PQ 的额外成本很容易暴露。** UpScaleDB 共享环境纯读，FC 和 FC-PQ 的 profile Jain 都约为 1，FC-PQ/FC 的 primary 吞吐配对中位数仅 0.397。多独立环境中，MCS、Ticket、CLH 等简单锁也构成强反例。
4. **USCL 的间歇与多锁退化真实存在，但不能包装为 FC-PQ 独有贡献。** FC、Native 和普通锁也能避开多环境中的数量级退化。H2 直接支持本地 USCL 保留时间片机制，但尚未定量解释真实 DB 全部延迟。
5. **公平性并未被实现为无条件保证。** UpScaleDB 八条批量写入下，FC-PQ profile Jain 仅约 0.628，读客户端只拿到约 11.5% 服务，远离四读四写的 50% 参考份额；USCL 同配置 Jain 约 0.908。持续同步请求不等于每次调度时都持续可选择。

因此，“公平几乎免费”“FC-PQ 总能胜过 MCS/USCL”“更公平自然意味着更低 p99”“收益已被证明来自缓存局部性”均不成立，或证据不足。

## 2. 实验账本与统计口径

| 实验族 | 已完成规模 | 目的与证据级别 |
|---|---:|---|
| 历史 saturn counter 基线 | 文档记录 16 锁、4–128T；原始重复数未核实 | 历史摘要，不与新数据合并 |
| 历史 demo：异质 CS、CS 长度、数组工作集、queue/PQ | 单次、1 秒、无 warmup | 演示观测，非正式稳定效应 |
| FC/FC-PQ 所有权与桥接正确性 | 多组 release/Miri、错误路径、churn、FFI 门 | 安全与正确性前置条件，不是性能样本 |
| LogP / 公平性模型检查 | 8 组有限检查 | 抽象模型验证，不是硬件测量 |
| 初始 UpScaleDB 集成 | 600 primary + 80 profile | 固定工作、120 秒持续模式、桥接对照 |
| admission accounting A/B | 200，100 配对 | counted/external 同二进制对照 |
| 物理核 / NUMA / SMT joined study | 330 primary | 17 fixed + 5 duration case，5 后端，3 次 |
| H1 操作角色与成本 | 54 首轮 + 18 串行 | 真实 DB 服务公平性与未插桩进展 |
| H2 保留时间片 | 60 首轮 + 60 串行 | 合成机制诊断 |
| H3 固定间歇请求 | 36 首轮 + 24 串行 | 人为 burst/sleep 敏感性实验 |
| 受控成本 / 独立到达 / DB 边界 / 多表 | 66 / 90 / 60 / 90 | 合计 306，四组独立边界研究 |
| 多表扩展其它锁 | 最终 180 | 10 后端、2 布局、3 表数、3 次 |
| 高竞争与热点路由 | 450 primary + 27 diagnostic | 10 后端、8/16/32W、3 路由、5 次 |
| UpScaleDB 异质客户端 | 126 primary + 126 profile | reads/single/batch × shared/split |
| redb 大小写事务 | 90 primary + 90 profile | 1/1、1/8、1/64 × Immediate/None |

上述数值研究（从初始集成起）合计 **2857 个进入最终分析的进程级试次**：680 + 200 + 330 + 252 + 306 + 180 + 909。这不是 2857 个同分布性能样本，也不是独立假设检验数；含 profile 与机制诊断。未计 smoke、正确性门、历史 demo、被保留但未纳入最终分析的整批尝试。

解释规则：

- **primary 与 profile 分开。** 吞吐、CPU 主要看未插桩 primary；服务 Jain 来自单独 profile，不能拼成同一次运行的 Pareto 点。
- **服务公平不等于次数公平。** Jain 衡量所选服务量的分配；服务量在这里通常是 callback/事务墙钟时间，可能包含抢占和 I/O，并非 CPU 时间。
- **固定工作与持续窗口不同。** fixed 完成相同读写数量；duration 会改变已完成 mix。读/s、写记录/s、事务/s不能相互替代。
- **三次/五次范围不是置信区间。** 部分报告以每次配对均超过 5% 作描述性筛选；混合结果不证明等价。初始十重复研究单独报告 nominal bootstrap CI，不与短研究混算。
- **延迟分位数多为直方图桶上界。** pooled p99 不代表最慢客户端；writer gap 是有限窗口观测极值，不是无饥饿上界。
- **CPU-s 不是能量。** 不同研究对 drain/join 的计时边界不同，按各自报告解释；不能跨模式随意相除。

## 3. 历史微基准：提出方向，但不是当前主证据

历史 demo 表（`experiment/upscaledb-fc-pq@7aa071d:docs/EXPERIMENT_RESULTS_DEMO.md`）覆盖异质临界区比例、临界区长度、counter-array 工作集、queue/PQ；设置仅 1 秒、1 次、无预热。历史 TODO（`experiment/upscaledb-fc-pq@7aa071d:TODO.md`）记载 saturn（Xeon Gold 6438M）counter，CS=[1000,3000]、non-CS=0、16 锁、4–128T：FC-PQ 在 128T 的 JFI=0.9996、吞吐约 FC 的 0.74 倍。

但 demo 自身有重要反例：4T 下 FC-PQ BHeap 的 JFI 随 1:1、1:3、1:10、1:30、1:100 分别为 1.0000、0.8346、0.5942、0.5349、0.5022。不能用概括性叙述覆盖这些表中观测，也不能以旧 128T 摘要保证新负载公平。

当前所查主线目录没有找回该历史基线的原始 Arrow/CSV，因而不能复算方差。临界区长度、数组等 demo 中的计数吞吐还依赖 benchmark 工作量定义，不能直接与 DB 请求 op/s横比。完整 ratio sweep、non-CS sweep、combiner-tail、NUMA/perf 等在 实验计划（`experiment/upscaledb-fc-pq@7aa071d:docs/EXPERIMENT_PLAN.md`）和 TODO 中仍有未完成项；脚本存在不等于已经执行。

**研究所得：** 早期结果足以提出“服务分配与执行位置可以分开考虑”的问题，不足以证明普遍公平或硬件缓存机制。

## 4. 正确性与理论验证：决定结果能否解释，不计为性能胜利

FC 节点所有权修复记录（`experiment/upscaledb-fc-pq-integration@dcbfacb:plan/2026-09-23/fc-node-ownership-repair.md`）覆盖已发布节点/公告环别名、统计内部可变性和队列扩展点；有 focused release/Miri、sealed-queue doctest 与 64/65/128-worker churn 等门。UpScaleDB 随后的真实数据库门检查错误传播、输出 buffer/canary、完整键集合、完整性、线程退出与关闭，及异常/panic 的 fail-stop 边界。

实验仅批准受限同步使用，不是通用 mutex 替换证明。早期 UpScaleDB 是内存 Store、固定键值、无事务/恢复/持久化；调用方 join 后独占销毁。后来的 redb 则真实提交完整写事务，不能混为同一种应用语义。

LogP 分析入口（`research/logp-analysis@555ad76:analysis/logp/README.md`）与模型检查结果（`research/logp-analysis@555ad76:.worktree/logp/verification.json`）提供性能和公平性分离的有限模型检查。8 组检查不是 Rust 线性化证明、硬件性能保证，也没有覆盖全文所有定理。概念插图不是实测曲线。

理论与实现之间仍需补齐：公告可见性、持续可选择集合、实际完成 batch、计费边界、combiner 返回与请求重提交流程。不能直接把有前提的 O(C_max) 结论贴到全部实测场景。

另外，FC-PQ admission-drain debug 检查候选（`experiment/fc-pq-implementation@90b89f8:plan/2026-09-24/fc-pq-admission-debug-check.md`）目前记录为改动已准备、验证与测量待做；编译器可能本就消除相关工作，不能宣称已有优化收益。它与第 6 节已经完成的 bridge 生命周期计数 A/B 是两件事。

## 5. 初始真实 UpScaleDB：既有正收益，也有强回退

来源：集成计划与完整结果（`experiment/upscaledb-fc-pq-integration@dcbfacb:plan/2026-09-23/upscaledb-integration-plan.md`）。固定工作每次 40 万 find + 40 万 insert；primary 每格十个新进程。比较 Native、重构原 mutex、bridge mutex、FC、FC-PQ，分离整体桥接与 delegation 差异。

### 5.1 固定工作与过量订阅

| 配置 | FC-PQ 对 Native 配对耗时加速均值 | 结论 |
|---|---:|---|
| packed，1 worker | 0.777× | 单线程变慢 |
| packed，2 workers | 0.909× | 未获益 |
| packed，4 workers | 2.039×，CI [1.653,2.421] | 条件性获益 |
| split，4 workers | 1.129×，CI [1.106,1.151] | 较小获益 |
| packed，8 workers | 0.494× | 四 CPU 上过量订阅，明显回退 |
| packed，16 workers | 0.209× | 回退加重 |
| split，16 workers | 0.157× | 更强反例 |

这里 packed/split 是早期 CPU 放置，不是后来“独立 environment”的 split。1/2-worker 两标签实际 CPU 子集相同，不能称作 NUMA 对照。

packed 8W 中，Native→refactored 耗时 −1.8%，refactored→bridge +10.5%，bridge→FC +54.2%，FC→FC-PQ +25.1%（均为配对均值，各自区间见来源）。桥接有成本，但不足以解释全部 delegation 回退。

### 5.2 120 秒持续模式

四个 finder + 四个 inserter、四 CPU，packed primary 均值：

| 后端 | find/s | insert/s | CPU-s / 120 秒 | 每次最差 writer gap 均值 |
|---|---:|---:|---:|---:|
| Native | 764690 | 573811 | 127.4 | 33.6 ms |
| FC | 292665 | 272638 | 479.9 | 19.1 ms |
| FC-PQ | 282031 | 229351 | 479.9 | 20.1 ms |

独立 profile 服务 Jain：bridge mutex 0.824、FC 0.852、FC-PQ 0.902；finder 服务份额为 27.1%、29.2%、33.6%。**更好的服务平衡并没有带来吞吐和 CPU 改善。** 两类各四人，FC-PQ 也没有达到等份服务。

插桩明显扰动运行：packed 8W fixed 中，FC 和 FC-PQ 耗时开销分别约 +32.4%、+20.7%。这要求之后所有公平性结论明确标注 profile。

## 6. admission accounting：一个被证据限制住的优化假设

200 次匹配 A/B（`experiment/upscaledb-fc-pq-integration@dcbfacb:plan/2026-09-24/upscaledb-admission-accounting.md`）在五后端、单 worker/同 socket 8W/跨 socket 8W/SMT 8W 下比较 counted 与 external。每格五配对，external/counted 吞吐比均值：

| 后端 | 单 worker | 同 socket | 跨 socket | SMT |
|---|---:|---:|---:|---:|
| FC | 1.189 | 1.149 | 1.442 | 1.590 |
| FC-PQ | 0.926 | 1.146 | 0.856 | 1.140 |
| USCL | 1.032 | 1.156 | 1.232 | 1.113 |

FC-PQ 方向不一致，大量范围跨 1；没有证明“admission 计数是 FC-PQ 主要瓶颈”。USCL 的吞吐与 CPU 对照更一致地有利于 external，但也不能推广为固定开销。

最终移除并发 stop/drain 协议，采用同步 execute + 调用方 join + 独占 destroy，**依据是 API 不需要该生命周期能力，而不是测得普遍提速**。旧 counted、临时 A/B 与新 joined 数据独立保存，不拿不同布局/预热条件的结果相减来分解开销。

## 7. 物理核、NUMA、SMT：相对更快，不等于可扩展

来源：330 次 joined study，含 12 图（`experiment/upscaledb-fc-pq-integration@dcbfacb:docs/reports/upscaledb-joined-dimensions/report.md`）。17 fixed case + 5 个 10 秒 duration case；五后端、每格三次。

固定 80 万操作、64 个物理 worker：

| 后端 | 吞吐 op/s，均值 | CPU-s | 对 Native 配对耗时加速 |
|---|---:|---:|---:|
| Native | 172859 | 12.00 | 1.00× |
| FC | 703665 | 72.74 | 4.07× |
| FC-PQ | 505970 | 84.66 | 2.93× |
| USCL | 563581 | 2.69 | 3.26× |
| CFL-local | 409571 | 117.95 | 2.37× |

FC 和 FC-PQ 的 P64/P1 吞吐留存只有 0.22×、0.20×；P1 又是单 worker 交替读写。**相对 Native 的高竞争优势不是线性强扩展，也不是核心效率证明。**

P64 duration 中，FC-PQ 相比 FC：find 580689 vs 338566/s，insert 236081 vs 338501/s；writer gap 5.363 vs 0.544 ms。总请求数可能增加，但完成 mix 向读移动，写入进展与极端间隔变差。

NUMA：FC balanced/compact 吞吐均值比在 P8 为 0.68、P32 为 0.74；但 socket1 远端执行/本地执行的 P8 比值为 1.38。远端并非总变慢，不能据此认定纯内存延迟因果。

SMT：同 W8 的 FC 比 0.96；同 C64 将 64 worker 增至 128 后为 1.09，USCL 为 0.89。同工人数与同核心预算是不同问题，不能合并成单一“SMT 有益/有害”。

## 8. H1–H3：把公平、保留时间片与间歇进展分开

来源：252 次假设实验，含 14 图（`experiment/upscaledb-fc-pq-integration@dcbfacb:docs/reports/upscaledb-hypotheses/report.md`）。首轮分区并行 150 次，随后串行确认 102 次；两批不混合统计。

### H1：不同成本的真实 DB 请求

8 个持续同步 worker，finder/inserter 为 7+1、1+7、4+4，每次 5 秒。insert/find profile 服务成本约 1.4–2.1 倍。三种比例下 FC-PQ 相比 FC 均改善服务 Jain 与角色份额误差；相比 USCL 则混合，4+4 串行未展示两指标同时改善。

串行 4+4 primary：FC 约 73.3 万 find/s、73.4 万 insert/s；FC-PQ 79.6 万、49.2 万；USCL 36.4 万、27.5 万。FC-PQ 改变的是分配，而不是全部角色共同提速。

### H2：直接验证保留时间片

A release 后不再进入锁，B 分别在目标间隔 0、50、500、5000 μs 发起请求。USCL 串行三个进程中位数的中位数等待约为 2180、2131、1681、0.582 μs；FC-PQ 对应约 0.601、0.664、0.681、0.647 μs。

在 A 仍持有效 reservation、B 未被 ban 的条件下，延迟追踪剩余时间片，到期后消失。直接状态观测来自单独本地 USCL 版本；冻结 bridge 的内部机理仍是推断。不能把这个机制实验变成全部 DB 延迟的定量归因。

### H3：人为 burst/sleep，只作为敏感性证据

4 find + 4 insert；bursty worker 每 64 次操作主动 sleep 5 ms。串行 one_per_role 中，USCL bursty 约 3264 op/s/worker、p99 桶上界 16.777 ms；FC 约 11800、0.016 ms；FC-PQ 约 11300、0.131 ms。

这说明该人工到达模式下 USCL 进展差，不证明真实应用中相同参数普遍存在。FC 与 FC-PQ 都有效，不能独归于公平 PQ。completion gap 含主动 sleep，不能当饥饿；half_per_role 没有独立串行确认。

## 9. 四组边界研究：从人工 sleep 走向更清楚的对照

来源：306 次边界研究，含 15 图及全部配置（`experiment/upscaledb-fc-pq-integration@dcbfacb:docs/reports/upscaledb-boundaries/report.html`）与机读 provenance（`experiment/upscaledb-fc-pq-integration@dcbfacb:docs/reports/upscaledb-boundaries/provenance.json`）。cost 66、arrival 90、database 60、tables 90，全部进入最终分析。准备可并行，正式计时通过全局锁串行；仍不保证整机隔离。

### 9.1 受控操作成本

合成 callback 计算量为 64/1024 iterations，覆盖 one_equal、eight_equal、four_four、seven_one；四后端、三次重复，48 primary + 18 独立 diagnostic。它隔离“操作本身变贵”这一输入，不是真实 DB 吞吐。

- one_equal：FC-PQ/FC 吞吐为 0.665–0.809，CPU/op 为 1.24–1.51 倍，p99 桶上界为 2 倍；单 worker 成本明确。
- eight_equal：吞吐方向混合，没有稳定胜出证据。
- four_four、seven_one：FC-PQ/FC 总请求吞吐分别约 1.20–1.52、1.35–1.91 倍，但 expensive worker 完成率下降，尾延迟更差；不能把更多便宜请求解释成相同工作提速。
- 非均匀成本的 profile 服务分配相对 FC 更平衡，但相比 USCL 更公平的主张不成立。

整体差异仍含调度、批次与完成 mix，不能当作独立 heap 开销测量。成本原始分析（`experiment/upscaledb-fc-pq-integration@dcbfacb:.worktree/upscaledb-boundaries/cost/analysis/report.txt`）保留逐角色结果。

### 9.2 独立到达

8 个稳定请求者，用预生成 Poisson 流覆盖读比例 95%/50%、共同参考容量的 0.3/0.7/1.1 倍，五后端、三次重复。延迟从计划到达时刻计算，避免遗漏提交前积压。参考容量基于 Native 校准并受共同负载设置约束；1.1 不是每种后端各自饱和容量的 1.1 倍。

- 95% 读、0.3 倍负载：各后端完成率几乎相同，没有额外吞吐收益；USCL 仍有较大的积压，完成率相同不表示延迟相同。
- 95% 读、1.1 倍：FC/FC-PQ 约完成 90.9 万/s，USCL 67.5–67.9 万/s；FC-PQ/USCL 配对完成率 1.338–1.348，USCL backlog 约 45.8–47.0 万。
- 50% 读、1.1 倍：FC/FC-PQ 约 45.83 万/s，bridge mutex 约 33.7–39.7 万/s，两者相对 bridge 三次均有完成率优势。但没有 FC-PQ 独有优势。

比固定 sleep 更适合检查负载与积压，仍是合成流而非生产 trace。应同时读取完成率、未完成与计划到达延迟，不能只看已完成样本 p99。到达原始分析（`experiment/upscaledb-fc-pq-integration@dcbfacb:.worktree/upscaledb-boundaries/arrival/analysis/report.txt`）保留 drain 与校准口径。

### 9.3 数据库工作集与竞争边界

预装载 1000/1000000 记录 × 1/8 worker，共四条件、五后端、三次重复，60 primary，无 service profile。单 worker 的 FC-PQ/FC 吞吐配对比，小读取域为 0.835–0.838，大读取域为 0.888–0.896；八 worker 中两种 delegation 均胜 Native，但 FC-PQ 不稳定胜 FC，大读取域三次均低于 FC。

这提供实际应用的低竞争成本边界，不提供服务公平性结论。工作集同时改变 DB 工作与缓存行为，不是受保护数据迁移的独立校准；不能跨 CPU 分区用绝对吞吐排名。数据库原始分析（`experiment/upscaledb-fc-pq-integration@dcbfacb:.worktree/upscaledb-boundaries/database/analysis/report.txt`）保留所有条件。

### 9.4 多表与独立 environment

8W、1/8/32 表、总初始记录 32768；coarse 共享一个 environment，split 每表独立 environment。不是绕过同一 environment 的必要锁。

USCL 8 表吞吐中位数从 coarse 597829 降到 split 4617 op/s；32 表从 590584.5 降到 9613。8 表 split 的 Native/FC/FC-PQ 约 332/382/345 万 op/s。USCL find 平均延迟约 1.728 ms，其他后端约 1.7–2.0 μs。

**真正结论是多锁访问暴露 USCL 的适配问题，而非 FC-PQ 独占优势。** K=1 coarse/split 语义相同，FC/FC-PQ 却存在大幅波动；原因未确定，必须保留该控制，不能把小差值当稳定胜负。未采内部锁占用时间线，不能把数量级退化全部归因于 reservation。

## 10. 把其它锁放进来：普通锁是必须保留的反例

来源：多表十后端同期重测（`experiment/multitable-other-locks@3f770d7:docs/reports/upscaledb-other-locks/report.html`）。Native、bridge mutex、FC、FC-PQ、USCL、CFL-local、SpinLock、MCS、Ticket、CLH；8 个物理核、1/8/32 表、coarse/split、2 秒、三次重复，最终 180/180。

split/32 吞吐中位数，Mop/s：CLH 8.740、SpinLock 8.550、Ticket 8.060、MCS 7.151、FC 6.207、FC-PQ 5.704、Native 5.670、CFL-local 3.142。MCS 与 CFL-local 等部分范围很宽，不以中位数做稳定全排序。

更可靠的配对方向：split/8 的 MCS、Ticket、CLH，以及 split/32 的 SpinLock、Ticket、CLH，三次均比 FC-PQ 快超过 5%。FC-PQ 对 FC 的六个配置都没有达到“三次均快超过 5%”。USCL split/8 和 split/32 仍只有约 4756、9304 op/s。

[INFERENCE] 单锁竞争减弱后，combining 可摊销收益下降，而发布、扫描、usage 与 PQ 等协议工作仍在。没有实际 batch/缓存跟踪，这只是与结果相容的解释，不是成本分解证明。

准入与失败记录不能删：

- ShflLock 因重叠不同宽度原子访问、共享引用写非 UnsafeCell 字段被排除；不换另一个实现冒充它。
- CFL-local 有跨句柄全局 accounting，未认证为论文原 artifact。
- CLH raw queue node 缺失回收；固定短进程中泄漏有界，但不能据此推荐长期反复创建锁。
- 首批 180 中 11 次触达插入容量上限；中间整批又有 11 次被控制器旧阈值拒绝。修正后整批重跑，最终只用最后 180，不挑失败点补样本。前两批保留。
- 8-worker 的 120 个多表门通过；额外 128-worker Ticket 首次 watchdog 超时、CLH 长耗时等诊断仍保留，不把它们藏在“全部测试通过”里。

## 11. 高竞争实验：相对优势出现的位置

来源：高竞争全十后端结果（`experiment/multitable-other-locks@3f770d7:.worktree/upscaledb-high-contention-validated/analysis-readable/report.html`）与最新三方向综合报告（`experiment/multitable-other-locks@3f770d7:docs/reports/fairness-campaign/report.html`）。32 独立环境、8/16/32 同 NUMA 物理 worker、uniform/hot90/hot100、2 秒、五配对；450 primary，另 27 API 重叠诊断。

| 32W 路由 | FC-PQ/MCS 吞吐配对中位数 [范围] | 解释 |
|---|---:|---|
| uniform | 0.743 [0.733,0.754] | 五次均劣于 MCS |
| hot90 | 1.135 [1.085,1.795] | 五次均有相对优势 |
| hot100 | 1.105 [1.092,1.662] | 五次均有相对优势 |

uniform 中 FC-PQ/MCS 绝对吞吐约 1045/1397 万 op/s；hot100 降至约 102/92 万。hot100 FC-PQ/FC 为 0.954，FC 仍略快。8W、16W 不构成所有热点均胜的规律。

两者 32W 都约 64 CPU-s/2 秒；hot100 CPU/完成 op 中位数 FC-PQ 31.4 μs、MCS 34.8 μs、FC 30.0 μs；uniform FC-PQ 3.06 μs、MCS 2.29 μs。

[INFERENCE] 集中竞争可能改善批次摊销和数据复用。但热点同时改变 B-tree 增长与工作集；API 重叠不是队长，没有实际每 pass 完成数或 cache migration。因此该结果证明条件性边界，不证明唯一局部性原因。

## 12. UpScaleDB 异质客户端：温和收益与重要失败案例

来源：异质客户端摘要（`experiment/heterogeneous-clients@d9f1abc:.worktree/heterogeneous-trials-20260925-01/analysis/summary.txt`）。8 同步客户端、2 秒、reads/single/batch、shared/split，126 primary + 126 profile。single/batch 为四读四写；batch 每次调度执行 8 条 insert、只取一次环境门，不是事务，失败可部分提交；Native batch 也是同边界补丁基线。

| 场景 | FC → FC-PQ profile 服务 Jain | primary 配对中位数 | 结论 |
|---|---|---|---|
| shared，single | 0.896 → 0.977 | 读 1.276×；写记录 0.711× | 服务重新分配，但 MCS 0.981、USCL 0.984 已很公平 |
| shared，batch8 | 0.579 → 0.628 | 读 0.841×；写记录 0.582×，范围跨1 | 公平改善有限，性能不稳，是重要反例 |
| shared，纯读 | 两者约 0.999–1.000 | 总吞吐 0.397× [0.394,0.604] | 几乎没有公平收益，却有成本 |
| split，batch8 | 各环境内 FC 已接近1 | 读 0.672×；写记录 0.741× | 跨类别竞争已消失，收益来源减弱 |

shared batch8 的读服务份额仅 7.3%→11.5%；USCL Jain 约 0.908。USCL 每窗口秒约耗 1.97 CPU-s，完成 46.7 万读/s、38.0 万写记录/s；FC-PQ 约 8.00 CPU-s，14.6 万读/s、81.6 万写记录/s。不存在不依赖目标函数的单一赢家。

[INFERENCE] 当前 FC-PQ 的公告吸收、已完成节点处理、combiner 必须结束 combine 才能返回重提交流程，可能使持续同步客户端不持续处于 selectable 集合。实际源码保留累计 usage，不能说“重入队就清零”。需要 selection/admission/return 时间线才能验证贡献，不能现在宣布唯一根因。

split 的公平域是每个环境内四人，不是八人的全局 50/50。profile 扰动也很大：batch/shared 的 FC-PQ profile/primary 请求吞吐比范围 0.732–1.867，不能把 profile Jain 当未插桩版本精确份额。

## 13. redb：目前最有解释力的应用证据

来源：redb 原始汇总（`experiment/redb-fair-transactions@e96f5f9:.worktree/redb-campaign-20260925-04/analysis/summary.json`）与三方向中文图文报告（`experiment/multitable-other-locks@3f770d7:docs/reports/fairness-campaign/report.html`）。redb 3.1.0，8 同步客户端，完整 begin_write→commit；不跨线程转移事务 guard，不额外制造数据库全局串行点。比较 Native、Mutex、MCS、FC、FC-PQ；事务大小 1/1、1/8、1/64；Immediate/None；各三次、2 秒，最终 180。

### 13.1 1/64 混合：服务公平的收益清楚

四个小事务客户端各写 1 条，四个大事务客户端各写 64 条。以下 Jain 与份额取 profile 中位数；吞吐比取 primary 三次配对比中位数，不视作同一个运行的联合点。

| durability | FC → FC-PQ 服务 Jain | 小事务服务份额 | 小事务吞吐比 [范围] | 总 records/s 比 [范围] |
|---|---|---|---:|---:|
| Immediate | 0.891 → 0.992 | 32.6% → 45.4% | 1.500 [1.492,1.567] | 0.832 [0.696,0.835] |
| None | 0.838 → 0.966 | 28.0% → 40.8% | 1.620 [1.608,1.768] | 0.826 [0.802,0.872] |

**解释：让小事务客户端拿到更多串行服务，以约 17% 的记录吞吐代价换取约 50%–62% 的小事务完成率提升。** 参考等份目标是 50%，并非完美均分。批量事务摊薄一次事务固定成本，完成 mix 改变后 records/s 的变化不能全归于 PQ 开销。

MCS 的对应 Jain 0.907/0.863：排队轮转不自动等于服务份额公平。不过在 1/1、1/8 时 MCS/FC 本已接近公平，FC-PQ 增益小；这些控制必须与 1/64 同时展示。

### 13.2 代价与公平指标之间的冲突

- 小事务 p99 桶上界中位数：Immediate FC 2.097 ms→FC-PQ 8.389 ms；None 1.049→4.194 ms。完成得更多，不等于尾部更低。
- FC、FC-PQ、MCS 每 2 秒约耗 15–16 CPU-s；Native/Mutex 通常 1.2–2.6 CPU-s。对真实 commit 等待，这是不可忽略的资源代价；没有细分周期数据就不能声称某个精确忙等百分比。
- Native 与 Mutex 各自 18 个 primary 配置都出现至少一个客户端整个 2 秒无完成。FC、MCS 未出现 250 ms 零进展窗；FC-PQ 在 18 个配置中有一个客户端一个 250 ms 零进展窗。FC-PQ 不是唯一能改善进展的方案，也不是无停顿方案。
- 服务计费含 I/O 与抢占，是事务执行机会的墙钟分配，不是 CPU 公平。Immediate 与 None 独立解释，None 不保证持久 commit；ext4 上 close/reopen 成功不是断电恢复验证。

初始完整 180 次中 7 次触达单 worker 100 万记录上限；统一提高至 800 万/worker 后完整重跑 180，最终无失败。旧失败保留，不纳入本节比例。最终数据库内容与 close/reopen oracle 通过记录不等同于崩溃一致性测试。

## 14. 从全部实验中学到了什么

### 14.1 三种收益必须分别论证

| 待论证内容 | 最合适的现有对照 | 不能偷换成 |
|---|---|---|
| delegation 是否值得 | 高竞争 FC/FC-PQ 对 MCS；物理核 fixed | 公平策略提高吞吐 |
| 公平调度是否改善分配 | redb 1/64 的 FC→FC-PQ；H1 与 single/shared | 所有客户端延迟都更低 |
| 成本是否值得 | records/s、读写速率、CPU、零进展与尾延迟同时看 | 单看 Jain 或总 op/s 排名 |

### 14.2 优势、无明显收益、劣势的地图

- **适合展示优势：** 成本明显异质、共享串行容量、客户端确有服务竞争的 redb 大小事务；足够高单门争用下，相对直接排队锁的 delegation 优势。
- **没有明显公平增益：** 同质纯读、小异质事务、已按独立环境分区、普通基线本身 Jain 已很高。
- **明显劣势或需要慎用：** 单线程/过量订阅，低单锁竞争，强调 CPU/能耗预算，强调 requester p99，UpScaleDB batch8 当前实现不能实现预期份额的场景。
- **不稳定或原因不足：** NUMA 某些反向结果、K=1 标签控制差异、部分三次配对跨 1、插桩强扰动。保留为不确定，不归为“无差异”。

### 14.3 建议采用的研究叙事

主结果选 redb 1/64：公平服务与小事务进展的收益，以及 records/s/CPU/p99 的明确代价。性能边界选高竞争 uniform→hot90/hot100，保留普通锁。应用反例保留 UpScaleDB 纯读、独立环境与 batch8。H2 作为机制诊断，H3 作为敏感性附录，不把人工 sleep 当主要现实性证据。

下一步最值得补的是实际 publication→visibility→selection→service→notification→return 时间线，以及每 pass 真实完成数、调度/计费边界。它能回答“为什么理想公平没有落到 batch8 上”，比继续挑好看的吞吐点更有价值。这是建议，不代表本报告已启动新实验。

## 15. 可追溯性与报告核查

所有入口已在各节链接。机器结果优先读取各自 analysis/summary.json；汇总报告的 provenance/reduced.json 给出源路径和 SHA-256。原始结果多在被 Git 忽略的 `.worktree/` 下，提交 Markdown 不等于备份原始证据。

本报告不宣称整机独占，不将不同分区、不同生命周期、不同预热、不同持续时间的数据合并。最终批次全部成功不意味着研发全过程无失败；容量保护、控制器校验遗漏、内存策略 smoke、超额订阅 watchdog 与安全准入失败均保留在各源记录中。

账本初版已核查：22 个本地报告链接均可解析，8 个源汇总 SHA-256 与 provenance/reduced.json 一致；四组边界 306、多表其它锁 180、最新三方向 909 的规模及 redb 180 条记录无失败已检查，2857 总数加法一致。未重新执行锁性能试验，不把文档核查称为新的算法验证。后续图文版另外完成图像、浏览器和 PDF 渲染检查，记录在其 verification.json。
