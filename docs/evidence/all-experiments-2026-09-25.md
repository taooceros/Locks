Copied 2026-09-26 from the experiment ledger (relative links rewritten as plain `branch@commit:path` text so nothing points outside this worktree), then pruned on 2026-09-28: sections 3, 6 and 13, section 5.2, the H3 part of section 8, all but the 4-worker cells of section 5.1, the one_equal cells of section 9.1, the single-worker cells of section 9.3 and the batch8 cells of section 12 were removed, and sentences that relied on them were cut. Section numbers follow the original. Reasons and locations of every removed study: [Withdrawn](README.md#withdrawn).
Verbatim original: `experiment/upscaledb-fc-pq-integration@dcbfacb:docs/reports/all-experiments-2026-09-25.md`.
Any `.worktree/` path below is a git-ignored raw artifact on the machine that ran the experiment; it is not in any branch.

# Locks 全部实验总结：FC-PQ 的价值、代价与适用边界

日期：2026-09-25（2026-09-28 剪裁）。范围：实现与数学验证，以及已完成、仍然有效的 UpScaleDB 实验。本文只汇总既有证据，没有重跑计时实验或修改锁算法；复制到多个 worktree 的报告只计一次。所有 FC-PQ 结果早于 `fcpq_fast_path`（E0(b)，PR #48）；所有保留的实验线程数均不超过 CPU 数。

## 1. 总结先行

**目前最有力的结论不是“FC-PQ 普遍更快”，而是：公平委托可以重新分配串行服务，使短请求客户端获得更多进展；收益取决于争用、请求异质性和实际可选择性，而且有吞吐、CPU 与返回尾延迟代价。**

主要发现：

1. **delegation 的性能优势有条件。** 32-worker 高竞争实验中，FC-PQ/MCS 从均匀访问的 0.743 倍变为 hot90 的 1.135 倍、hot100 的 1.105 倍。但制造热点使 FC-PQ 自己的吞吐从约 1045 万降至 102 万 op/s；相对领先不等于系统变快。
2. **无明显服务失衡时，FC-PQ 的额外成本很容易暴露。** UpScaleDB 共享环境纯读，FC 和 FC-PQ 的 profile Jain 都约为 1，FC-PQ/FC 的 primary 吞吐配对中位数仅 0.397。多独立环境中，MCS、Ticket、CLH 等简单锁也构成强反例。
3. **USCL 的间歇与多锁退化真实存在，但不能包装为 FC-PQ 独有贡献。** FC、Native 和普通锁也能避开多环境中的数量级退化。H2 直接支持本地 USCL 保留时间片机制，但尚未定量解释真实 DB 全部延迟。

因此，“公平几乎免费”“FC-PQ 总能胜过 MCS/USCL”“更公平自然意味着更低 p99”“收益已被证明来自缓存局部性”均不成立，或证据不足。

## 2. 实验账本与统计口径

| 实验族 | 已完成规模 | 目的与证据级别 |
|---|---:|---|
| FC/FC-PQ 所有权与桥接正确性 | 多组 release/Miri、错误路径、churn、FFI 门 | 安全与正确性前置条件，不是性能样本 |
| LogP / 公平性模型检查 | 8 组有限检查 | 抽象模型验证，不是硬件测量 |
| 初始 UpScaleDB 集成 | 600 primary + 80 profile（整个研究；仅保留 4-worker 格子） | 固定工作、桥接对照 |
| 物理核 / NUMA / SMT joined study | 330 primary | 17 fixed + 5 duration case，5 后端，3 次 |
| H1 操作角色与成本 | 54 首轮 + 18 串行 | 真实 DB 服务公平性与未插桩进展 |
| H2 保留时间片 | 60 首轮 + 60 串行 | 合成机制诊断 |
| 受控成本 / 独立到达 / DB 边界 / 多表 | 54 / 90 / 30 / 90 | 合计 264，四组独立边界研究 |
| 多表扩展其它锁 | 最终 180 | 10 后端、2 布局、3 表数、3 次 |
| 高竞争与热点路由 | 450 primary + 27 diagnostic | 10 后端、8/16/32W、3 路由、5 次 |
| UpScaleDB 异质客户端 | 84 primary + 84 profile | reads/single × shared/split |

表中规模是各研究进入最终分析的进程级试次，含 profile 与机制诊断；不是同分布性能样本，也不是独立假设检验数。未计 smoke、正确性门、被保留但未纳入最终分析的整批尝试。

解释规则：

- **primary 与 profile 分开。** 吞吐、CPU 主要看未插桩 primary；服务 Jain 来自单独 profile，不能拼成同一次运行的 Pareto 点。
- **服务公平不等于次数公平。** Jain 衡量所选服务量的分配；服务量在这里通常是 callback/事务墙钟时间，可能包含抢占和 I/O，并非 CPU 时间。
- **固定工作与持续窗口不同。** fixed 完成相同读写数量；duration 会改变已完成 mix。读/s、写记录/s、事务/s不能相互替代。
- **三次/五次范围不是置信区间。** 部分报告以每次配对均超过 5% 作描述性筛选；混合结果不证明等价。初始十重复研究单独报告 nominal bootstrap CI，不与短研究混算。
- **延迟分位数多为直方图桶上界。** pooled p99 不代表最慢客户端；writer gap 是有限窗口观测极值，不是无饥饿上界。
- **CPU-s 不是能量。** 不同研究对 drain/join 的计时边界不同，按各自报告解释；不能跨模式随意相除。

## 4. 正确性与理论验证：决定结果能否解释，不计为性能胜利

FC 节点所有权修复记录（`experiment/upscaledb-fc-pq-integration@dcbfacb:plan/2026-09-23/fc-node-ownership-repair.md`）覆盖已发布节点/公告环别名、统计内部可变性和队列扩展点；有 focused release/Miri、sealed-queue doctest 与 64/65/128-worker churn 等门。UpScaleDB 随后的真实数据库门检查错误传播、输出 buffer/canary、完整键集合、完整性、线程退出与关闭，及异常/panic 的 fail-stop 边界。

实验仅批准受限同步使用，不是通用 mutex 替换证明。早期 UpScaleDB 是内存 Store、固定键值、无事务/恢复/持久化；调用方 join 后独占销毁。

LogP 分析入口（`research/logp-analysis@555ad76:analysis/logp/README.md`）与模型检查结果（`research/logp-analysis@555ad76:.worktree/logp/verification.json`）提供性能和公平性分离的有限模型检查。8 组检查不是 Rust 线性化证明、硬件性能保证，也没有覆盖全文所有定理。概念插图不是实测曲线。

理论与实现之间仍需补齐：公告可见性、持续可选择集合、实际完成 batch、计费边界、combiner 返回与请求重提交流程。不能直接把有前提的 O(C_max) 结论贴到全部实测场景。

另外，FC-PQ admission-drain debug 检查候选（`experiment/fc-pq-implementation@90b89f8:plan/2026-09-24/fc-pq-admission-debug-check.md`）目前记录为改动已准备、验证与测量待做；编译器可能本就消除相关工作，不能宣称已有优化收益。

## 5. 初始真实 UpScaleDB：既有正收益，也有强回退

来源：集成计划与完整结果（`experiment/upscaledb-fc-pq-integration@dcbfacb:plan/2026-09-23/upscaledb-integration-plan.md`）。固定工作每次 40 万 find + 40 万 insert；primary 每格十个新进程。比较 Native、重构原 mutex、bridge mutex、FC、FC-PQ，分离整体桥接与 delegation 差异。

### 5.1 固定工作（线程数不超过 CPU）

| 配置 | FC-PQ 对 Native 配对耗时加速均值 | 结论 |
|---|---:|---|
| packed，4 workers | 2.039×，CI [1.653,2.421] | 条件性获益 |
| split，4 workers | 1.129×，CI [1.106,1.151] | 较小获益 |

这里 packed/split 是早期 CPU 放置，不是后来“独立 environment”的 split。

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

## 8. H1–H2：把公平与保留时间片分开

来源：假设实验报告（`experiment/upscaledb-fc-pq-integration@dcbfacb:docs/reports/upscaledb-hypotheses/report.md`）。本节保留 H1、H2 共 192 次；首轮分区并行，随后串行确认，两批不混合统计。

### H1：不同成本的真实 DB 请求

8 个持续同步 worker，finder/inserter 为 7+1、1+7、4+4，每次 5 秒。insert/find profile 服务成本约 1.4–2.1 倍。三种比例下 FC-PQ 相比 FC 均改善服务 Jain 与角色份额误差；相比 USCL 则混合，4+4 串行未展示两指标同时改善。

串行 4+4 primary：FC 约 73.3 万 find/s、73.4 万 insert/s；FC-PQ 79.6 万、49.2 万；USCL 36.4 万、27.5 万。FC-PQ 改变的是分配，而不是全部角色共同提速。

### H2：直接验证保留时间片

A release 后不再进入锁，B 分别在目标间隔 0、50、500、5000 μs 发起请求。USCL 串行三个进程中位数的中位数等待约为 2180、2131、1681、0.582 μs；FC-PQ 对应约 0.601、0.664、0.681、0.647 μs。

在 A 仍持有效 reservation、B 未被 ban 的条件下，延迟追踪剩余时间片，到期后消失。直接状态观测来自单独本地 USCL 版本；冻结 bridge 的内部机理仍是推断。不能把这个机制实验变成全部 DB 延迟的定量归因。

## 9. 四组边界研究

来源：边界研究，含全部配置（`experiment/upscaledb-fc-pq-integration@dcbfacb:docs/reports/upscaledb-boundaries/report.html`）与机读 provenance（`experiment/upscaledb-fc-pq-integration@dcbfacb:docs/reports/upscaledb-boundaries/provenance.json`）。本节保留 cost 54、arrival 90、database 30、tables 90，共 264 次。准备可并行，正式计时通过全局锁串行；仍不保证整机隔离。

### 9.1 受控操作成本

合成 callback 计算量为 64/1024 iterations，覆盖 eight_equal、four_four、seven_one；四后端、三次重复，36 primary + 18 独立 diagnostic（four_four、seven_one）。它隔离“操作本身变贵”这一输入，不是真实 DB 吞吐。

- eight_equal：吞吐方向混合，没有稳定胜出证据。
- four_four、seven_one：FC-PQ/FC 总请求吞吐分别约 1.20–1.52、1.35–1.91 倍，但 expensive worker 完成率下降，尾延迟更差；不能把更多便宜请求解释成相同工作提速。
- 非均匀成本的 profile 服务分配相对 FC 更平衡，但相比 USCL 更公平的主张不成立。

整体差异仍含调度、批次与完成 mix，不能当作独立 heap 开销测量。成本原始分析（`experiment/upscaledb-fc-pq-integration@dcbfacb:.worktree/upscaledb-boundaries/cost/analysis/report.txt`）保留逐角色结果。

### 9.2 独立到达

8 个稳定请求者，用预生成 Poisson 流覆盖读比例 95%/50%、共同参考容量的 0.3/0.7/1.1 倍，五后端、三次重复。延迟从计划到达时刻计算，避免遗漏提交前积压。参考容量基于 Native 校准并受共同负载设置约束；1.1 不是每种后端各自饱和容量的 1.1 倍。

- 95% 读、0.3 倍负载：各后端完成率几乎相同，没有额外吞吐收益；USCL 仍有较大的积压，完成率相同不表示延迟相同。
- 95% 读、1.1 倍：FC/FC-PQ 约完成 90.9 万/s，USCL 67.5–67.9 万/s；FC-PQ/USCL 配对完成率 1.338–1.348，USCL backlog 约 45.8–47.0 万。
- 50% 读、1.1 倍：FC/FC-PQ 约 45.83 万/s，bridge mutex 约 33.7–39.7 万/s，两者相对 bridge 三次均有完成率优势。但没有 FC-PQ 独有优势。

这适合检查负载与积压，但仍是合成流而非生产 trace。应同时读取完成率、未完成与计划到达延迟，不能只看已完成样本 p99。到达原始分析（`experiment/upscaledb-fc-pq-integration@dcbfacb:.worktree/upscaledb-boundaries/arrival/analysis/report.txt`）保留 drain 与校准口径。

### 9.3 数据库工作集与竞争边界

预装载 1000/1000000 记录 × 8 worker，共两条件、五后端、三次重复，30 primary，无 service profile。两种 delegation 均胜 Native，但 FC-PQ 不稳定胜 FC，大读取域三次均低于 FC。

八 worker 结果提供实际应用的竞争边界，不提供服务公平性结论。工作集同时改变 DB 工作与缓存行为，不是受保护数据迁移的独立校准；不能跨 CPU 分区用绝对吞吐排名。数据库原始分析（`experiment/upscaledb-fc-pq-integration@dcbfacb:.worktree/upscaledb-boundaries/database/analysis/report.txt`）保留所有条件。

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

## 12. UpScaleDB 异质客户端：single 与纯读

来源：异质客户端摘要（`experiment/heterogeneous-clients@d9f1abc:.worktree/heterogeneous-trials-20260925-01/analysis/summary.txt`）。8 同步客户端（CPU0–7）、2 秒、reads/single × shared/split，84 primary + 84 profile。single 为四读四写。

| 场景 | FC → FC-PQ profile 服务 Jain | primary 配对中位数 | 结论 |
|---|---|---|---|
| shared，single | 0.896 → 0.977 | 读 1.276×；写记录 0.711× | 服务重新分配，但 MCS 0.981、USCL 0.984 已很公平 |
| shared，纯读 | 两者约 0.999–1.000 | 总吞吐 0.397× [0.394,0.604] | 几乎没有公平收益，却有成本 |

split 的公平域是每个环境内四人，不是八人的全局 50/50。

## 14. 从全部实验中学到了什么

### 14.1 三种收益必须分别论证

| 待论证内容 | 最合适的现有对照 | 不能偷换成 |
|---|---|---|
| delegation 是否值得 | 高竞争 FC/FC-PQ 对 MCS；物理核 fixed | 公平策略提高吞吐 |
| 公平调度是否改善分配 | H1 与 single/shared | 所有客户端延迟都更低 |
| 成本是否值得 | records/s、读写速率、CPU、零进展与尾延迟同时看 | 单看 Jain 或总 op/s 排名 |

### 14.2 优势、无明显收益、劣势的地图

- **适合展示优势：** 足够高单门争用下，相对直接排队锁的 delegation 优势。
- **没有明显公平增益：** 同质纯读、已按独立环境分区、普通基线本身 Jain 已很高。
- **明显劣势或需要慎用：** 低单锁竞争，强调 CPU/能耗预算，强调 requester p99。
- **不稳定或原因不足：** NUMA 某些反向结果、K=1 标签控制差异、部分三次配对跨 1、插桩强扰动。保留为不确定，不归为“无差异”。

### 14.3 建议采用的研究叙事

研究叙事以仓库根目录 README.md 的研究计划为准。性能边界选高竞争 uniform→hot90/hot100，保留普通锁。应用反例保留 UpScaleDB 纯读与独立环境。H2 作为机制诊断。

下一步最值得补的是实际 publication→visibility→selection→service→notification→return 时间线，以及每 pass 真实完成数、调度/计费边界。这是建议，不代表本报告已启动新实验。

## 15. 可追溯性与报告核查

所有入口已在各节链接。机器结果优先读取各自 analysis/summary.json；汇总报告的 provenance/reduced.json 给出源路径和 SHA-256。原始结果多在被 Git 忽略的 `.worktree/` 下，提交 Markdown 不等于备份原始证据。

本报告不宣称整机独占，不将不同分区、不同生命周期、不同预热、不同持续时间的数据合并。最终批次全部成功不意味着研发全过程无失败；容量保护、控制器校验遗漏、内存策略 smoke、超额订阅 watchdog 与安全准入失败均保留在各源记录中。

原始账本的核查记录（报告链接、源汇总 SHA-256、各研究规模）见分支上的原文；本剪裁版只删除撤回内容，没有重新核查源汇总，也没有重跑锁性能试验。
