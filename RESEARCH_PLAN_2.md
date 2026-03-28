Below is a full research plan built around the stronger thesis you want: **delegation is not merely a place to add fairness; it is the first locking setting where fairness can be enforced without paying the usual handoff-locality cost as the dominant term.**

# Research Plan

## Working title

**Breaking the Fairness–Performance Tradeoff with Usage-Fair Delegation Locks**

Alternative title:

**Virtual-Runtime Combining: Usage-Fair Delegation Without Shared-State Handoff**

---

## 1. Core thesis

**Main thesis.** In contended shared-state workloads, delegation largely breaks the conventional fairness–performance tradeoff. In a handoff-based lock, making the policy fair changes which thread executes the next critical section, so fairness directly perturbs where the shared working set lives. In a delegation lock, the combiner remains the executor, so fairness mostly changes **which request** the combiner serves next, not **which core** owns the critical section. That means the dominant penalty of fair handoff—repeated inter-core migration of lock-protected state—is removed from the fairness decision. What remains is primarily combiner-local scheduling overhead, requester-input locality, and operation-order effects inside the combiner’s own cache hierarchy. Flat Combining already established the key premise: a single combiner executes announced operations, the core data structure is accessed sequentially, synchronization overhead is very low, and cache misses can be far lower than prior techniques; TCLocks reinforces the same execution model in a transparent-delegation setting; ShflLock shows why locality-sensitive lock scheduling matters in handoff locks at all. 

This paper should therefore sell a strong but precise claim:

> **Fair delegation breaks the fairness–performance tradeoff at the dominant cost term.**
> Traditional fair locks pay by moving ownership. Fair delegation pays mostly by doing local scheduling.

That is the center of the paper. Everything else should support it.

---

## 2. Problem statement

Delegation locks such as Flat Combining and the broader combining/delegation family deliver high throughput because one combiner thread batches or executes requests on behalf of waiters, reducing synchronization and coherence traffic on hot shared state. But the service policy in these locks is not usage-fair by default. FIFO service or list order may be acquisition-fair, yet still allocate disproportionate total lock time to threads with longer critical sections or favorable arrival patterns. U-SCL gives the right framing for why that matters: lock policy can determine who effectively runs, thereby subverting the scheduler’s intended allocation of CPU progress and lock opportunity. 

There are really two paper-worthy problems here. First, **usage unfairness**: a delegation lock can preserve throughput while giving unequal shares of lock-protected service time to equal-priority threads. Second, **combiner asymmetry**: the combiner can suffer disproportionate response time because it executes its own request plus a batch of other threads’ requests before it can resume, a cost model that is structurally different from ordinary waiter latency. TCLocks explicitly highlights this type of asymmetry as a resource-accounting and thread-identity problem when a combiner executes on behalf of others. ([Research Wisconsin][1])

---

## 3. Research questions

The plan should answer four questions.

**RQ1.** Can a delegation lock provide strong usage fairness, not merely FIFO acquisition fairness?

**RQ2.** Does delegation actually change the fairness–performance tradeoff relative to handoff-based fair locks?

**RQ3.** What mechanism is best inside delegation: virtual-runtime ordering, banning/admission control, or a hybrid?

**RQ4.** How much combiner-specific latency remains after fairness is added, and can combiner handoff policy reduce it?

---

## 4. High-level approach

The paper should be centered on **one main design** and **one contrast design**.

### 4.1 Main design: FC-VR

The primary algorithm should be **FC-VR**: Flat Combining with **virtual lock runtime** scheduling. Earlier notes called this FC-PQ; the paper should rename it to emphasize the policy, not the data structure.

Each thread `i` has a cumulative virtual runtime `vr_i`. The combiner maintains a local priority structure keyed by `vr_i` and always serves the waiting thread with minimum virtual runtime. After serving a request with measured critical-section cost `cs_i`, the combiner updates:

[
vr_i \leftarrow vr_i + \frac{cs_i}{w_i}
]

where `w_i` is the thread’s weight. Equal-weight fairness is the default case; weighted fairness becomes a natural extension rather than a separate redesign.

The priority structure should be a **combiner-owned binary heap** in the main implementation. It fits the paper’s argument: fairness scheduling is local, sequential, and contention-free. A B-tree or skip list can remain implementation notes or appendix material, not the headline algorithm.

### 4.2 Contrast design: FC-Ban

The second design should be **FC-Ban**, a non-work-conserving baseline that enforces fairness via temporary exclusion. After request `i` consumes service time `cs_i`, the combiner computes a cooldown proportional to recent usage and current contention, for example:

[
banned_until_i \leftarrow now + \alpha \cdot cs_i \cdot W
]

where `W` is the number of waiters and `\alpha` is a tunable constant. This is intentionally strict and may leave throughput on the table, but it tests the reviewer question directly: *is reordering enough, or do you need admission control to enforce fairness?*

That question is not hypothetical. SynCord reports that reorder-only policies can incur many policy violations, while backoff-based admission control reduces them substantially, because reordering cannot stop a thread that arrives before the victim even joins the queue. 

### 4.3 Why this pairing is enough

This pairing gives the paper a clean story:

* **FC-VR** shows that fair delegation can stay fast because fairness is paid as local scheduling.
* **FC-Ban** shows the enforcement extreme and answers the “reordering alone may not suffice” objection.

Everything else—CC-Ban, FC-SL, transparent integration, hierarchical weights—should be treated as stretch goals or future work unless they are essentially free to add.

---

## 5. Lock abstraction and implementation model

The programming model remains a delegation API rather than a mutex API:

```text
lock(input) -> output
```

A thread publishes a request, the combiner executes the delegate function on shared state, and the result is returned. This matches Flat Combining’s publication-list model and the broader delegation family. QD Lock is useful to cite here as evidence that delegation-style APIs and libraries are a real design space, not a one-off artifact of Flat Combining. 

The implementation plan should be:

1. Waiters publish requests into a lock-free announcement structure.
2. The combiner drains announced requests into a local heap keyed by `vruntime`.
3. The combiner pops the minimum-`vruntime` request, executes it, measures service time with TSC or a stable cycle counter, updates accounting, publishes the result, and repeats.
4. The heap and accounting structures remain combiner-owned, so scheduling itself adds no inter-thread contention.

Two policy refinements are important from the start.

**Newcomer initialization.** A newly active thread must not start at zero. CFL’s key insight is to initialize new arrivals relative to current system progress, using virtual lock hold time inspired by CFS-like virtual runtime. FC-VR should initialize a new thread at current `min_vruntime` or a nearby average/median to avoid letting newcomers leapfrog long-waiting threads merely because they have no history yet. ([ACM Digital Library][2])

**Starvation bound.** If a thread has been skipped too many times or falls too far behind ideal service, clamp its `vruntime` deficit or force temporary priority boost. This converts an empirical fairness story into a bounded-lag story.

---

## 6. Paper claims and analysis plan

The paper should make three technical claims.

### 6.1 Correctness claim

Linearizability follows from the underlying delegation model: the combiner already applies operations sequentially, and Flat Combining’s correctness argument depends on the fact that at most one combiner executes `scanCombineApply` at a time. FC-VR changes **service order**, not the basic linearization mechanism. 

### 6.2 Dominant-cost claim

This is the paper’s analytical heart:

> For two waiting requests served by the same combiner during the same combining epoch, swapping their service order does not change the identity of the executor. Therefore, any cost difference induced by fairness must come from request-fetch locality, combiner scheduling overhead, or shared-state overlap inside the combiner’s cache hierarchy—not from inter-core transfer of the shared working set.

That statement is partly a new argument, but it is grounded by existing facts: Flat Combining centralizes access to the shared data structure and benefits from lower coherence and cache-miss traffic; TCLocks explicitly frames delegation as shipping the critical section rather than the data; ShflLock exists precisely because handoff order changes locality and cache-line movement in queue locks. 

### 6.3 Fairness claim

The fairness target is **usage fairness**, not acquisition fairness. For equal weights, each active thread should converge toward an equal share of lock-protected service time over the measurement window. For weights, the target becomes proportional service. U-SCL provides the right conceptual precedent with lock opportunity and scheduler alignment; CFL provides the right algorithmic precedent with virtualized lock-time accounting. ([Research Wisconsin][1])

---

## 7. Positioning against prior work

The related-work section should be blunt and simple.

**Flat Combining and combining locks.** Flat Combining showed that a single combiner can drastically reduce synchronization overhead and cache misses by sequentializing access to the shared structure. CC-Synch and DSM-Synch extended the combining family and remain important delegation-style baselines, but they do not address usage fairness as a first-class objective. 

**U-SCL.** U-SCL established the scheduler-subversion problem, introduced lock opportunity as the fairness quantity, and showed that explicit usage control can align lock service with scheduler goals. But its slice-based design is non-work-conserving and can waste the lock when the current slice owner is outside the critical section. That makes it the right conceptual ancestor and the right baseline, but not the right mechanism for delegation. ([Research Wisconsin][1])

**CFL.** CFL is the closest fairness paper. It treats lock occupation as a schedulable resource and uses virtual lock hold time inspired by `vruntime`. The paper should acknowledge CFL as the algorithmic inspiration for virtualized fairness accounting while arguing that delegation changes where scheduling lives: CFL schedules over a waiter queue in a handoff lock; FC-VR schedules inside the combiner. ([ACM Digital Library][2])

**ShflLock and SynCord.** ShflLock shows off-path queue reordering for locality and groups same-socket waiters while enforcing policy mostly off the critical path. SynCord generalizes this design space and, importantly, reports that reorder-only policies can fail to strictly enforce a desired policy compared with backoff/admission-control variants. That motivates FC-Ban and makes FC-VR vs. FC-Ban a meaningful comparison rather than a gratuitous extra design. ([Systems Software and Security Lab][3])

**TCLocks.** TCLocks solves transparency, not fairness. It shows that delegation can be made practical with zero application changes, lightweight context switching, and ephemeral stacks, but it also explicitly identifies resource-accounting and thread-identity complications when a combiner runs code on behalf of other threads. That makes TCLocks complementary: your paper answers how to make delegation fair; TCLocks answers how to make delegation transparent. 

**QD Lock.** QD Lock is worth including to show that delegation APIs and delegation libraries are a broader family. Your novelty is not “delegation exists”; it is “fairness belongs inside delegation, and delegation changes the cost of fairness.” ([IEEE Computer Society][4])

---

## 8. Evaluation plan

### 8.1 Systems under test

Use two machines. The primary machine is your 2-socket Sapphire Rapids server. The second should be an AMD EPYC system or CloudLab equivalent. That is enough to make the locality story credible across vendor/cache-topology differences.

### 8.2 Baselines

Keep the baseline set tight:

* **Unfair delegation:** FC, one CC-Synch/DSM-Synch representative.
* **Your designs:** FC-VR, FC-Ban.
* **Traditional unfair/fair locality baselines:** MCS, ShflLock, CFL, U-SCL.

This is enough. More lines will blur the graphs.

### 8.3 Workloads

Use four workload groups.

**Group A: heterogeneous critical sections.** Shared counter or simple shared object with service ratio sweep from 1:1 to 1:100 and non-critical-section sweep from saturated to lightly contended. This is where usage unfairness should show up most directly.

**Group B: data-footprint and access-pattern sweep.** The counter-array benchmark is essential. Vary array size from L1-resident to clearly beyond LLC-friendly footprints, and run both sequential and random access. This experiment is the empirical test of the main thesis. Sequential access is the honest null result; random access is the decisive result.

**Group C: combiner-role latency.** Instrument response time separately for combiner-served and waiter-served operations. Report overall and role-split CDFs. Add a combiner-handoff policy experiment such as tail-as-combiner.

**Group D: one credible application benchmark.** The best candidate is a delegation-friendly concurrent hash map with `get`, `put`, and `scan` operations and a Zipfian distribution. `scan` gives the long-service tail, while `get` and `put` give the short-service majority. If time permits, add a producer/consumer log buffer. If you can make LevelDB work, it will help greatly, but it should not block submission.

### 8.4 Metrics

Use five primary metrics:

* total throughput
* per-thread serviced lock time
* Jain’s fairness index
* p50/p95/p99/p99.9 response time, overall and role-split
* PMU counters: cache misses, LLC misses, dTLB misses, and cross-socket traffic if available

Per-thread serviced lock time should be the main fairness plot. JFI is the compact table metric.

### 8.5 Methodology

Pin threads. Use warmup. Use repeated trials with confidence intervals. For development, 15-second runs are fine; for the paper, move to 30-second runs for throughput and shorter dedicated runs for latency distributions. Keep the methodology consistent across all locks.

### 8.6 Key figures

There should be five paper figures that matter most.

1. **Central figure:** throughput vs. fairness frontier.
   This is the paper in one picture. FC-VR should lie near the top-right frontier.

2. **Heterogeneous-service fairness bars:** per-thread serviced lock time under 1:10 and 1:30 ratios.
   This makes usage unfairness visually obvious.

3. **Sequential vs. random footprint plot:** throughput of FC, FC-VR, CFL, and ShflLock/MCS across working-set size.
   This validates the main locality claim and includes the null result honestly.

4. **Combiner vs. waiter latency CDFs.**
   This isolates delegation-specific latency asymmetry.

5. **Overhead waterfall:** FC → FC + local scheduler → FC-VR + newcomer initialization → FC-VR + starvation bound → FC-VR + prefetch.
   This proves where the overhead actually comes from.

### 8.7 Success criteria

The plan succeeds if you can show three things:

* **Fairness:** FC-VR substantially improves serviced-time fairness over FC/CC-style FIFO combining.
* **Tradeoff claim:** FC-VR loses much less throughput relative to FC than CFL loses relative to handoff baselines in the same contended, random-access regimes.
* **Mechanism claim:** Most of FC-VR’s overhead is local scheduler overhead, not remote queue-policy enforcement or shared-state migration.

---

## 9. Risks and mitigation

The biggest risk is that **ultra-short critical sections** make local scheduling overhead visible enough to drown the fairness story. That is not fatal. It just means the paper should explicitly scope its claim to contended shared-state workloads where service cost is not microscopic. Flat Combining itself already notes that there are regimes where fine-grained parallelism wins, especially when the combined work does not amortize the sequential bottleneck. 

The second risk is that **sequential access patterns** allow prefetching to hide locality differences. That is why the sequential/random split must stay in the paper. A null result is not a weakness here; it sharpens the scope condition.

The third risk is **newcomer bias**. Solve it early with `min_vruntime`-style initialization.

The fourth risk is **application credibility**. If a full application retrofit is too invasive, be candid: the paper evaluates delegation-compatible workloads that isolate the phenomenon of interest. TCLocks can be the future-work bridge to transparent deployment. 

The fifth risk is **strict fairness enforcement**. If FC-VR occasionally allows transient bully bursts, that is exactly why FC-Ban exists in the paper.

---

## 10. Timeline

A realistic 16-week plan is:

Weeks 1–2: finalize fairness metric, implement FC-VR core, and add accurate per-request timing.

Weeks 3–4: implement newcomer initialization, starvation bound, and prefetch; bring up FC-Ban.

Weeks 5–6: baseline cleanup for FC, MCS, ShflLock, CFL, and U-SCL; unify harness and pinning.

Weeks 7–8: heterogeneous-CS and array-footprint experiments; generate the central frontier figure.

Weeks 9–10: combiner-role latency instrumentation and handoff-policy experiments.

Weeks 11–12: application benchmark and PMU analysis.

Weeks 13–14: write design, evaluation, and related work.

Weeks 15–16: tighten claims, reduce scope if necessary, and polish for submission.

---

## 11. Venue strategy

**Primary target: PPoPP.** The topic sits directly in parallel programming, synchronization, runtime behavior, and multicore performance, which is squarely within PPoPP’s scope. **Fallback: EuroSys.** If the application and systems story becomes stronger, EuroSys is a very good fit. **Second fallback: USENIX ATC.** If the paper ends up reading more like a practical systems design with extensive evaluation than a synchronization-theory paper, ATC is also appropriate. ([ppopp25.sigplan.org][5])

---

## 12. Final paper pitch

If I had to compress the whole plan to two sentences for the introduction, I would use this:

**Locks traditionally face a fairness–performance tradeoff because fair service order changes lock ownership and therefore data locality. Delegation changes that equation: once a combiner owns execution, fairness becomes a local scheduling problem, so a usage-fair delegation lock can approach unfair-delegation throughput while delivering scheduler-aligned lock service.**

That is the version of the paper I would write.

[1]: https://research.cs.wisc.edu/wind/Publications/eurosys20-scl.pdf "Avoiding Scheduler Subversion using Scheduler–Cooperative Locks"
[2]: https://dl.acm.org/doi/10.1145/3627535.3638477?utm_source=chatgpt.com "Fairly Scheduling Lock Occupation with CFL"
[3]: https://gts3.org/assets/papers/2019/kashyap%3Ashfllock.pdf "Scalable and Practical Locking with Shuffling"
[4]: https://www.computer.org/csdl/journal/td/2018/03/08093701/13rRUxlgy3o?utm_source=chatgpt.com "Queue Delegation Locking"
[5]: https://ppopp25.sigplan.org/track/PPoPP-2025-Main-Conference-1?utm_source=chatgpt.com "PPoPP 2025 - Main Conference"
