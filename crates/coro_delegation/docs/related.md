# Related work for `../RESEARCH.md` (F1 combiner burden, F2 bystander delay, F3 cost-heterogeneous service fairness)

`§` = section; `p.` = page of the fetched PDF (the arXiv HTML has no pages); code is `file:line@commit`. **[INF]** = my inference; n/r = not reported.

**1. CES** (König, Epple, Becker, arXiv:2511.09194v1).
*Taxonomy (§3.2):* there are two ways to hand the lock to the next waiter.
- *Inline*: `unlock` resumes the waiter recursively, which causes "collapse of parallelism" (§4.2).
- *Dispatch*: `unlock` enqueues the waiter and the owner continues (Kotlin, Boost, Tokio). This causes queuing delay (§4.1).

*Mechanism (§5, Fig. 6):* `unlock` does `schedule(current)` to *another* thread, then `waiter.resume()` here. Ordering is FIFO. The unlocking thread combines "until contention drops" (§2). A TAS spinlock guards the state word and intrusive waiter list (§5.1). *Batch bound:* none (Fig. 6).
*Remark (§5 "Comparison…", Fig. 7; §2):* on *preemptive* delegation locks, work on the combiner thread "is delayed … This delay can be arbitrarily long". §7 concedes that the unlocker's continuation "might wait in the ready queue".
*Fairness:* none claimed. The only use is a "fair reader-writer mutex" (§6.2).
*Measured:* throughput only.
- Setup: 2-socket AMD (128 cores) and 4-node Intel (64 cores); median of 5 runs (§6). Workload: 5000 tasks × 1000 iterations of `map.insert` plus a sieve (§6.1, Fig. 8).
- 8.1× over dispatch at 256 threads (0.124 → 1.01 Mops/s). About 10× for CS ≤250 ns and about 2× at 7.5 µs (Fig. 10).
- RW 1.55× / 1.23× (§6.2). Java −6…−18 % (CS <1 µs) and +37 % (7.5 µs) (§6.3).
- LevelDB 3.3× TCLocks (§6.4). Dispatch queuing delay median 35.5 µs vs a 319 ns CS (§6.5). §5.4 checks thread placement only qualitatively.

*Not addressed:* per-thread combiner share (F1); non-lock latency and any tail metric (F2); only uniform CS cost is tested (F3).

**2. TCLocks** (Gupta et al., OSDI'23).
*Mechanism:* the waiter parks its registers and SP and spins on an ephemeral stack. The combiner runs its CS on the waiter's stack (§3.1, p.5).
*Ordering:* TAS + MCS queue. The head that wins the TAS combines if ≥2 waiters, walking the queue in order (Listing 1, p.7) with same-socket waiters first (§3.6.3, p.9–10).
*Batch bound:* a count "to limit … starvation or long-term fairness issues" (§3.2, p.5–6), `WAITERS_TO_COMBINE=1024` (p.7). Observed about 950–980 waiters per combiner (§6.1, p.12) and up to 50,000 in RCUHT (p.13).
*Fairness:* "…short-term fairness, but TCLocks maintain long-term fairness" (§6.3, p.15). No metric.
*Measured:* throughput on 224 cores (§6, p.12); CS avg/p99 188/474 ns (p.13); p99.99 lock+CS+unlock (Fig. 5b).
*Not addressed:* combiner cost (F1). §7 (p.15) defers "account[ing] to the waiter thread". There are no bystanders because waiters spin (F2), and no cost mix (F3).

**3. Flat Combining** (Hendler et al., SPAA'10).
*Mechanism:* requests go in publication records. The CAS winner runs one `scanCombineApply` pass, in list order with the newest at the head; the others spin (§2, p.3–4).
*Batch bound:* one pass ∝ active threads (§1.1, p.1–2). "number of combining rounds a combiner performs consecutively" is only a knob (§7, p.9).
*Fairness:* linearizable and starvation-free (Lemmas 1–2, §5, p.6). §7 suggests stronger cores get "a preference in acquiring the global lock".
*Measured:* throughput, CAS/op and L2 misses/op on Niagara and Nehalem (§6, Fig. 2).
*Not addressed:* the combiner's own delay (F1). F2 and F3 are absent.

**4. Tokio `sync::Mutex`** (docs 1.53.1; src `4d71b41`).
*Mechanism:*
- It is `Semaphore::new(1)` (`sync/mutex.rs:363`) with `push_front` / `pop_back` (`sync/batch_semaphore.rs:516,327`). The permit is handed to the queue head, then the waker fires (`:327-366`).
- A wake from a worker lands in that worker's LIFO slot (runtime docs; `worker.rs:1392-1406`). The slot is polled next and cannot be stolen (`:1159`).
- The slot does not reset the coop budget (128, `coop/mod.rs:116`) and is capped at 3 in a row against "starvation" (`worker.rs:269,763`).
- **[INF]** So this is a bounded inline-after-poll hand-off on the unlocking worker, not pure dispatch as CES claims.

*Fairness:* "guaranteed FIFO". Per #6049, it "refers to the case where many tasks are waiting to lock it … not … tasks not using the mutex". The runtime promises only an eventual `MAX_DELAY` and "no guarantee … equally fair to all tasks" (runtime docs).
*Measured:* n/r.
*Not addressed:* there is no combiner (F1). F2 is out of scope by statement. FIFO ignores cost (F3).

**5. async-task 4.7.1** (`src/runnable.rs@f98b30b`).
- `run` polls once, then the `Runnable` "vanishes and only reappears when its `Waker` wakes the task" (l.755-761).
- `schedule` gets `ScheduleInfo{woken_while_running}` (l.78-100) and "should not attempt to run the `Runnable`" (l.355-357).
- **[INF]** The executor holds the sole handle, so `schedule_inline` = a per-worker slot drained after `run()` returns, without re-entrancy.
- No fairness claims and no locks, so nothing on F1–F3.

**6. FC dispatcher** (Lee, Yoon, Moon, EuroSys'25 poster).
*Mechanism:* an idle worker wins the queue lock, "distributes requests to all workers (including itself)", then goes back to being a worker (§2, p.1). Ordering and bound: n/r. *Fairness:* none claimed.
*Measured:* median and P99 latency and throughput on RocksDB. Up to 84 % better P99 and +14 % throughput (§3–4, p.2). Machine: n/r.
*Not addressed:* the role rotates (an F1 mitigation), but its cost is not measured. F2 and F3 are absent.

**7. Shinjuku** (NSDI'19) and **Perséphone** (SOSP'21): heterogeneity handled at the scheduler.
- **Shinjuku:** a centralized dispatcher preempts every 5–15 µs via posted IPIs (§3.1–3.2, p.5–7).
  - Policies: SQ = FCFS with re-enqueue. MQ = per-type queues, chosen by queue-time/SLO ratio (§3.4, p.7–8).
  - Lock ordering is out of scope: `call_safe` disables preemption, and contested locks "scale poorly regardless of scheduling policy" (§3.6, p.9–10).
- **DARC:** profiles per-type cost and reserves cores for short types, which may steal from longer types but not the reverse. It is non-work-conserving (§3, p.4–5).
  - It targets a 10× slowdown SLO "for each type of requests" (§2, p.3).
  - It assumes requests "are not dependent on each other" (§5.1, p.8), so lock contention is outside its model.

## Gap statement

| | CES | TCL | FC | Tokio | a-task | FC-disp | Shinjuku | DARC |
|---|---|---|---|---|---|---|---|---|
| F1 burden | partial | partial | partial | none | none | partial | none | none |
| F2 bystander | partial | none | none | none | none | none | none | none |
| F3 cost fairness | none | none | none | none | none | none | partial | partial |

- **F1: noticed but never measured.** CES observes the CS "remains on a dedicated thread per resource" (§5.4). TCLocks bounds batches (p.7) and defers accounting (§7). FC proposes combiner preference (§7). The poster rotates the dispatcher (§2). None reports a per-thread share, so H-A and H-D are untested.
- **F2: named only by CES, and only qualitatively.** CES's "arbitrarily long" remark targets *preemptive* delegation locks (§5), and its own trade-off is conceded without a number (§7, §6). Tokio's 3-poll cap is an unmeasured mitigation, and Tokio scopes fairness to waiters (#6049). H-B is untested.
- **F3: solved at the request scheduler, never inside a lock.**
  - Shinjuku and DARC exclude contested locks (§3.6) and dependent requests (§5.1).
  - TCLocks asserts long-term fairness without a metric (§6.3).
  - CES, FC and Tokio are FIFO or list-ordered and are evaluated at uniform CS cost.
  - No source tests usage-ordered combining (FC-PQ, H-C) or whether it survives inline resume.
