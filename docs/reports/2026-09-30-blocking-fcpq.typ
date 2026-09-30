// Report: blocking FC-PQ (wake-on-pick). Written for Typst HTML export;
// also compiles to PDF. View: gistd with ?g-output=html&g-version=latest.
#set document(title: [Blocking FC-PQ: wake-on-pick keeps throughput, loses fairness on mixed requests])
#set table(stroke: 0.5pt + gray, inset: 5pt)
#show table.cell.where(y: 0): strong

#let inference = [*[INFERENCE]*]

= Blocking FC-PQ: wake-on-pick keeps throughput, loses fairness on mixed requests

2026-09-30.
Plan: `plan/2026-09-30/fcpq-block-wake-on-pick.md`.
Code: jj change `tyzntutr` (feature `block_park`, on E0(a) `uwqronmm`); redb port `mzztyvkorznr` (on `mnkkkmky`).

== Summary

- We gave FC and FC-PQ blocking waiters that follow TCLocks' blocking lock: the waiter parks, and the combiner wakes it when it _picks_ its request.
- On redb with identical transactions (all1, 4–8 clients), parking immediately matches spinning FC-PQ's throughput (0.99–1.01×). It cuts CPU time from 8.00/15.99 to 3.95/3.99 CPU-s, the same as u-SCL. Service Jain is 0.974–1.000.
- On mixed transactions (half1_half64) it fails at every client count: throughput 0.75–0.89× spin and service Jain 0.654–0.841, against 0.938–0.994 spinning and 1.000 for u-SCL.
- Blocking favours the *long*-transaction clients. At 8 clients a 64-record client gets 0.203 of the lock's service time and a 1-record client 0.047 (fair share 0.125).
- #inference Mechanism: a parked client comes back late, and FC-PQ can only put a client first if its request is waiting.
  - The combiner starts a client's wake-up when it picks its request, so the client has one transaction's length to wake up. That is enough for a 93 µs transaction but not for a 33 µs one.
  - FC-PQ is work-conserving and keeps no credit for absence, so the time a late short client misses goes to whoever is waiting.
- Consequence for the paper: spinning is not inherent to FC-PQ for uniform workloads. For mixed request lengths, blocking brings back the idleness cost in the price-of-fairness framework, on the FC-PQ side (lost share) instead of the u-SCL side (an idle lock).

== 1. Design

=== Prior blocking policies

#table(
  columns: 4,
  [Lock], [When a waiter parks], [Who wakes it, when], [Source],
  [TCLocks, userspace], [Immediately after one check (no spin budget)], [Combiner, when it picks the request (`wake_up_waiter` before `execute_cs`)], [`rs3lab/TCLocks` `src/userspace/litl/src/kombmtx.c`],
  [TCLocks, kernel], [Spins until `need_resched()`; yields if it is the only runnable task, else parks], [Same], [`src/kernel/rcuht/locks/komb_mutex.c:42-66`; OSDI'23 §3.4],
  [ShflLock-B], [Spins until its time quota expires, then CAS `S_WAITING → S_PARKED`], [The shuffler, off the critical path, wakes waiters it moves forward; lock stealing hides wake-ups], [SOSP'19 §4.2.2],
  [CFL mutex], [Spin-then-park (Karlin et al. 1991)], [Proactively wakes waiters it reorders; barging allowed only for threads meeting its socket and vLHT conditions], [PPoPP'24 §3.3 (`docs/related-work/CFL-PPoPP24.txt:886-907`)],
)

=== `block_park`

- *One state word per request:* `WAITING`, `PARKED`, `PICKED`.
  - The owner resets it to `WAITING` before publishing the request.
  - The waiter spins for `DLOCK_SPIN_BEFORE_PARK_US`, then CASes `WAITING → PARKED` and calls `futex_wait`. If the CAS fails, the request has been picked, and the waiter spins on `complete`.
  - The combiner swaps in `PICKED` and calls `futex_wake` only if the old value was `PARKED`.
  - Both sides write the same word, so no Dekker fence is needed, the wake is not deferred, and there is no timeout.
- *Wake point:*
  - FC: when the scan reaches the node.
  - FC-PQ: when the request is popped from the heap, before its delegate runs.
  - Optional lookahead (`DLOCK_WAKE_LOOKAHEAD=1`, FC-PQ only) also wakes the request at the top of the heap.
- *Unlock hand-off:* unchanged from E0(a). The releasing combiner keeps combining while parked waiters have pending requests.
- `PICKED` is terminal for a request: a picked waiter never re-parks. This is a deviation from the plan; re-parking would need a second wake-up after completion.

=== Correctness

- `cargo test --release -p libdlock`: 155 passed / 3 ignored, plus the doc test, each under default, `spin_park` and `block_park`.
- dlock2 tests under `block_park` with a 5 µs budget and with budget 0: 54 passed. These include `idle_holder_release` and the `thread_churn` tests ported to FC, FC-PQ heap and FC-PQ btree.
- Loom (`RUSTFLAGS="--cfg loom"`, `dlock2::park::loom_model`): 5 scenarios pass.
  - The scenarios: waiter vs combiner pick, idle holder, retirement, lookahead at the pass limit, and stale state after service.
  - A sixth, three-thread scenario is ignored. Loom 0.7 never reschedules a runnable thread there; this is not a protocol bug.
- Every new line is behind `#[cfg(feature = "block_park")]`. A one-time normalised `objdump` diff of 120 FC/FC-PQ functions shows the default build is unchanged.
- `strace` at budget 0 (`idle_holder_release`): 480 futex waits, 493 wakes, 336 of which hit a sleeper, 2 `EAGAIN`.
- redb port with `block_park,fcpq_fast_path`: 63 dlock2 tests passed.

== 2. Microbenchmark

- Benchmark: `d-lock2 counter-proportional --non-cs 0`, 1 s warm-up plus 3 s measured, 3 interleaved runs per cell.
- Environment: CPUs 16–23 under the measurement `flock`, clock fixed at 3.0 GHz (checked after each run).
- 66 cells, 198 runs, no hangs.
- Data: `.worktree/output/fcpq-block/micro/`; full table in the plan.

FC-PQ (`fc-pq-b-heap`), medians, as Mops/s and CPU-s:

#table(
  columns: 8,
  [Config], [cs], [Spin], [`spin_park` 100 µs], [block 0], [block 5 µs], [block 100 µs], [block 0, lookahead 1],
  [8T / 8 CPUs], [1000], [1.10, 32.0], [1.02, 28.5], [0.59, 13.8], [0.84, 20.3], [1.17, 30.3], [0.61, 16.9],
  [8T / 8 CPUs], [20000], [0.071, 32.0], [0.058, 18.2], [0.059, 7.9], [0.059, 8.9], [0.070, 18.9], [0.059, 13.7],
  [8T / 4 CPUs], [1000], [0.55, 16.0], [0.58, 15.3], [0.20, 6.7], [0.56, 10.4], [0.52, 10.2], [0.14, 7.4],
  [32T / 8 CPUs], [1000], [0.30, 32.1 (Jain 0.33)], [0.52, 23.2], [0.27, 9.0], [0.23, 11.6], [0.52, 17.2], [0.24, 12.0],
  [32T / 8 CPUs], [20000], [0.019, 32.1 (Jain 0.31)], [0.036, 16.6], [0.031, 6.1], [0.029, 6.4], [0.024, 12.8], [0.025, 10.5],
)

- *Short critical sections, one CPU per thread:* parking immediately loses 46% (FC 64%), at about 440k context switches per second. Every request pays a full sleep and wake cycle.
- *Long critical sections:* the E0(a) collapse (`spin_park` 0.058) is gone at budget 100 µs (0.070 vs spin 0.071). Budget 0 costs 17% throughput for 75% less CPU.
- *Oversubscribed:* every blocking variant beats pure spinning on fairness. Pure spinning drops to Jain 0.31–0.33 at 32T.
- *Lookahead 1:* buys nothing (at most +3%) and costs more CPU.

== 3. redb

- Setup: S1 at 3.0 GHz (`--check-power` passed), CPUs 16–23, 2000 ms, 3 repeats, `none` durability, FC-PQ with `fcpq_fast_path`.
- The spinning FC-PQ and u-SCL references were rerun the same day and match the archive (c8 CPU-s 15.99 vs 3.95).
- 216 cells, 0 failed. Of these, 17 are outside the 3.0 GHz ±2% band; they are kept.
- Data: `~/Locks-artifacts/fcpq-block-redb/`.

#table(
  columns: 6,
  [Cohort], [c], [FC-PQ spin: tx/s, CPU-s, Jain], [block 0: tx vs spin, CPU-s, Jain], [block 100 µs: tx vs spin, CPU-s, Jain], [u-SCL: tx vs spin, CPU-s, Jain],
  [all1], [2], [32,473, 4.00, 1.000], [0.92, 2.98, 1.000], [0.99, 4.00, 1.000], [0.98, 4.00, 1.000],
  [all1], [4], [30,991, 8.00, 1.000], [1.01, 3.95, 1.000], [1.03, 6.45, 0.942], [1.01, 3.99, 1.000],
  [all1], [8], [32,398, 15.99, 0.999], [0.99, 3.99, 0.974], [1.03, 8.30, 0.941], [0.95, 3.95, 1.000],
  [half1_half64], [2], [17,476, 4.00, 0.938], [0.89, 3.37, 0.841], [0.99, 3.98, 0.940], [1.16, 3.99, 1.000],
  [half1_half64], [4], [19,049, 8.00, 0.994], [0.83, 3.98, 0.836], [0.70, 6.55, 0.702], [1.03, 4.00, 1.000],
  [half1_half64], [8], [18,626, 15.99, 0.953], [0.75, 4.02, 0.654], [0.79, 6.59, 0.695], [1.04, 3.95, 1.000],
)

The plan's acceptance criteria, at budget 0, are throughput within about 5% of spin, CPU-s near u-SCL's, and Jain ≥ 0.94.
- all1 passes at 4 and 8 clients.
- half1_half64 fails at every client count.
- Budget 100 µs saves little CPU, because waiters rarely wait longer than 100 µs when every thread has its own core. It also fails half1_half64 at 4 and 8 clients.

== 4. Who loses under blocking

Per-client service share, computed from `service_tsc_ticks` in each run's `result.json`, half1_half64. Each value is the median over the clients of a class and over 3 repeats. The fair share per client is 1/c.

#table(
  columns: 6,
  [c], [Variant], [1-record client share], [64-record client share], [tx per client (1-rec / 64-rec)], [Service per tx (1-rec / 64-rec)],
  [2], [FC-PQ spin], [0.372], [0.628], [21,468 / 13,481], [34 / 92 µs],
  [2], [FC-PQ block 0], [0.282], [0.718], [15,708 / 15,240], [35 / 91 µs],
  [4], [FC-PQ spin], [0.231], [0.269], [13,562 / 5,488], [34 / 96 µs],
  [4], [FC-PQ block 0], [0.147], [0.364], [8,738 / 7,572], [33 / 92 µs],
  [8], [FC-PQ spin], [0.106], [0.145], [6,414 / 3,056], [33 / 94 µs],
  [8], [FC-PQ block 0], [*0.047*], [*0.203*], [*2,806 / 4,249*], [33 / 94 µs],
  [8], [u-SCL], [0.124], [0.126], [7,106 / 2,506], [34 / 99 µs],
)

- Blocking shifts service to the 64-record clients. This is why records/s rises (1.12–1.38× spin) while tx/s falls.
- At 8 clients the 1-record clients complete _fewer_ transactions than the 64-record clients, although each of their transactions uses a third of the lock time.
- The 1-record clients have the lowest accumulated usage, so FC-PQ should always serve them first. Their median response time is still 2#super[17] ns (131–262 µs), several long transactions' worth.
- The FC-PQ fast path is not involved: 0–1 hits per client at 8 clients.
- Spinning FC-PQ already has a small bias the same way (0.372 vs 0.628 at c2, Jain 0.938). Blocking magnifies it.

== 5. Mechanism #inference

FC-PQ reorders only requests that are present. A client whose request is not enrolled when the combiner chooses is passed over. FC-PQ charges it nothing for the pass-over and credits nothing for it afterwards.

With wake-on-pick, the waiter starts waking when its request is picked. It therefore has its own transaction's service time, _c_, to become runnable before it can submit the next request. Let _L_ be the time from `futex_wake` to the next enrollment. It includes wake-up, scheduling, a possible C-state exit (S1 pins frequency, not C-states), the return, and preparing the next transaction. Then:

- A client is back on time when _c_ ≥ _L_. Otherwise it is absent for _L_ − _c_ after every transaction.
- While it is absent, the work-conserving combiner serves some waiting client, and that client's usage grows instead. The absent client cannot catch up: its time share is capped at roughly _c_ / (_L_ + _W_), where _W_ is the residual of the transaction in progress when it re-enrols.
- For _L_ between about 33 and 93 µs, 64-record clients (_c_ ≈ 93 µs) are always back on time, and 1-record clients (_c_ ≈ 33 µs) are late every cycle. This matches the data:
  - all1 is fine because every client pays the same _L_;
  - the skew grows with client count because there is always a waiting long client to absorb the gap.
- The microbenchmark cs 1000 losses are the same effect with no one to absorb it: _c_ is far below _L_, so every waiter is late and the lock idles.

u-SCL avoids this by _not_ being work-conserving. The slice owner keeps the lock across its own gaps between transactions, so a late client loses nothing and the lock idles instead. Its Jain is 1.000 in every cell.

Blocking FC-PQ and u-SCL therefore sit on the two sides of the same trade-off in the price-of-fairness framework's idleness term:
- A work-conserving lock gives away the share of absent clients.
- A lock that holds for absent clients idles.

Spinning FC-PQ escapes this only because a spinning waiter re-enrols within a few hundred cycles.

Not measured, so these remain open:
- _L_ itself.
- Per-client enrollment gaps.
- Park and wake counts on redb (the harness does not record voluntary context switches).

== 6. Implications

- *Intro claim* ("spinning is an implementation choice… delegation locks can park their waiters"): supported for uniform request lengths. On all1, blocking FC-PQ reaches u-SCL's CPU time at spinning throughput.
- *Mixed request lengths:* the current blocking policy breaks usage fairness. §8 must state this, and the price-of-fairness section should use it as the delegation-side instance of the idleness cost.
- *Not a verdict on blocking in general:* the policy wakes too late for short requests. Fixes to test:
  + *Time-based early wake:* wake a waiter once the predicted service time ahead of it in the heap falls below _L_. FC-PQ already keeps per-client usage estimates. Lookahead 1 by position was too little.
  + *Bounded absence credit:* charge an absent client less, up to a cap. This trades some work conservation for fairness, a partial step towards u-SCL.
  + *Spin for about one pass, then park:* spin if the expected wait is short. Budget 100 µs did not fix fairness, so the budget alone is not enough.

== 7. Next measurements

+ S2 (C6 disabled on CPUs 16–23; the user applies it): if _L_ shrinks, the 1-record share should move back towards 1/c.
+ Add to the redb harness a per-client counter for the enrollment gap (completion to next enroll) and per-client park and wake counts. The gap should be near zero for 64-record clients and positive for 1-record clients.
+ Implement fix 1 and rerun half1_half64 at c2/c4/c8 alongside spin and u-SCL.

== Artifacts

#table(
  columns: 2,
  [What], [Where],
  [Implementation, tests, loom logs, variant builds], [`.worktree/output/fcpq-block/` (`BUILD-VARIANTS.md`, `tests/`)],
  [Microbenchmark raw data, `summary.csv`, driver], [`.worktree/output/fcpq-block/micro/`],
  [redb raw data (`ref-spin-uscl`, `block-b100`, `block-b0`)], [`~/Locks-artifacts/fcpq-block-redb/`],
  [Plan and per-phase results], [`plan/2026-09-30/fcpq-block-wake-on-pick.md`],
)
