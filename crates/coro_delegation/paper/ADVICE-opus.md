# PC review: "Fair Locks for Coroutine Runtimes: Ordinary Wakers Buy Fairness, Executor Awareness Buys Speed"

Reviewer: OpusPaperAdvice (read-only). Draft at d99b41f0 (`paper/paper.typ`, 11 pp.).
Evidence checked: `FINDINGS.md` (top entry "ordinary-waker co-pq, same-window matrix"
and the entries below it), `REVIEW-2026-09-30.md` (I1–I10), `RESEARCH.md` ("API
assumption", binding), `src/locks/co_mutex.rs`, `src/executor.rs`, and the 132
`results/co2-*.json` files, which I re-analysed. The per-class throughput, lock-busy
fraction, bystander poll cost and foreign-CS share below are my own computations from
those JSONs (medians of 3). Anything marked [INFERENCE] was not measured.

Line numbers refer to `paper.typ`. Quotes are from the rendered `paper.pdf`.

---

## 0. Ranked summary (most important first)

| # | Issue | Severity | Where |
|---|---|---|---|
| 1 | The headline cost (co-pq o ≈ 5771) comes from this executor's wake discipline, and the draft presents it as a property of ordinary wakers. The arithmetic o ≈ P + t_B fits all four ordinary-wake numbers. tokio's ordinary wake already runs the grantee next (o = 1340). So the title's second clause is untested on any real runtime | invalidates the title as a general claim | title, L60, L77, L202, L218, L267 |
| 2 | "Ordinary wakers buy fairness" is close to a tautology: grant order is internal to the lock. The count of "10 cells" includes 4 bursty cells where Jain 0.999 is a symptom of slowness, and there co-pq is Pareto-dominated by the "unfair" locks | overclaim | L60, L77, L200, L267 |
| 3 | Novelty over SCL/u-SCL + CES + FC is thin. The genuinely new results (the burden metric plus the K-bound/home-break fix, the rule that a clamp is counted in the lock's own tick, async unlock as the step-aside point) are small and are not the headline | contribution | §1, §8 |
| 4 | Two of the three "forms of subversion" (async-lock monopoly, std/parking_lot seizure) were measured only in the spin harness, which the paper itself calls an artefact for the LIFO case. REVIEW I1 showed that std-mutex seizure disappears under yield | inconsistent evidentiary standard | L71, L113, Fig. 1b |
| 5 | Known contrary evidence is left out: the sleep-world inversion of the placement recommendations (REVIEW I4), and the fact that a usage-ordered *inline* hand-off was already fair and fast (co1 inline co-pq; REVIEW dpq-inline) | selective reporting | L192, L218, L237 |
| 6 | The "model predictions" are identities or back-fits. FIFO Jain "to three digits" follows from equal ops. The clamp-8 "prediction 0.9395" uses a 0.28 that was back-solved from the measured L:H | overclaim | L71, L81, L109, L152–154 |
| 7 | The workload cannot carry an OSDI/SOSP claim: TSC-spin CS, spinning bystanders that use 64–81 % of every CPU, closed loop, one lock, W ≤ 16, no application, no open-loop latency, and the uncontended path is never taken | rejection-level method gap | §2.3, §7 |
| 8 | The metrics hide what matters. o is throughput restated. Service Jain rewards Pareto-worse outcomes. Foreign-CS burden Jain is 1.000 for co-pq even though 90 % of its CS cycles run on a foreign worker | measurement | §2.3, Q1, Q2 |
| 9 | Draft hygiene: 11 red TODOs, a "What we withdrew" section, references to "an earlier version" and "the review", internal dates in every caption, and one TODO that is stale (TSan and the allocation probe for co_mutex are in FINDINGS) | presentation, double-blind | L77, L173, L180, L218, L241–247 |
| 10 | Cumulative, undecayed usage. G1 is rescoped to always-present clients, which removes the realistic case | scope | L127, L237 |
| 11 | The async-release result comes from another session (co1), and under yield it is worth only 2–9 % | weak evidence | L212 |
| 12 | Small numeric and wording errors ("within 5 %" is 5.1 %; tokio "equals" the inline locks in bursty cells but is in fact 8–11 % faster; "needs 256" was never bracketed; c0 and c256 are counted as separate cells) | minor | L60, L208, L224, L230 |

**Verdict:** reject at OSDI/SOSP, weak reject at EuroSys (reasons in §6). **Single
most important next experiment:** run the unchanged ordinary-waker `co-pq` on
tokio's multi-thread runtime, with the LIFO slot on and off, in both harness modes,
beside `tokio::sync::Mutex`. Pair it with a flag-only sweep of P and t_B on the coro
executor to test the o ≈ P + t_B mechanism (§6).

---

## 1. Is there a real, crisp contribution?

### The thesis as written

"A lock in a coroutine runtime is a hidden scheduler. Fairness needs only a
usage-ordered grant order with ordinary wakers; speed needs executor-aware mechanisms
(inline hand-off, combining)."

### The thesis the evidence supports

"In a cooperative runtime, a lock's grant order fixes each client's share of lock
time, and where the grantee lands relative to the releaser's continuation fixes the
lock's speed. On an executor whose default wake appends to the back of the local
queue, every hand-off waits one run-queue rotation. An inline hand-off or combining
avoids that wait, and tokio's LIFO slot gives it to ordinary wakers already."

The first half ("grant order decides share") is correct. It is also nearly
definitional: which waiter the lock grants next is decided inside the lock, under its
spinlock (L144), before any waker is touched. No reviewer will be surprised that a
min-usage heap gives equal usage whatever the wake mechanism. The part that is not
obvious is the second half, and there the draft measured only one executor for its
own locks. By its own admission, that executor's ordinary wake differs from tokio's
(L230: "in tokio the ordinary wake is already an inline hand-off, so the cost of
co-pq may be a property of our executor rather than of ordinary wakers").

### Novelty against the named prior work

| Prior work | What it already has | What this paper adds | Increment |
|---|---|---|---|
| SCL / u-SCL (EuroSys '20) | "Scheduler subversion"; usage accounting; lock slices; u-SCL is the non-delegating usage-ordered lock; its §7 suggests usage-fair delegation | The cooperative-executor setting; "where" as a second decision; a hand-off clamp counted in the lock's own tick | Small. co-pq is u-SCL without slices (the draft says so at L254), and the missing slices are exactly why G1 had to be rescoped (issue 10) |
| CES (arXiv 2511.09194) | Inline resume for coroutines; the observation that the scheduler round trip is on the critical path | Burden metric; chain bound K plus a home break; `ces-k64-home` gives burden Jain 0.993–0.999 at 0.98–0.99× CES throughput (L190) | **The paper's cleanest new result.** Well measured and cheap. It is not the headline |
| FC (SPAA '10) | Combining; a knob for consecutive rounds | FC-PQ (usage-ordered pass), yield-after-combine, remote wakes | Small. FC-PQ mirrors `libdlock`'s `fc_pq` (RESEARCH.md l.61), so it is presumably the group's earlier work. The draft correctly disclaims the combination (L254) |
| TCLocks (OSDI '23) | Transparent delegation; batch bound of 1 024 "for long-term fairness" | Measures per-worker combiner share | Small, and correctly stated (L256) |
| tokio | FIFO semaphore; wake through the LIFO slot (capped at 3); coop budget | Shows that under yield, tokio's mutex matches the inline locks (o = 1340), i.e. tokio already has "executor awareness" for ordinary wakers | This **undercuts** the title rather than adding to it |

**Missing related work a PC will name:**
- Go's `sync.Mutex` starvation mode: a direct hand-off after 1 ms, which is fairness inside the lock in a cooperative runtime. Go's `runnext` slot for readied goroutines is the closest analogue of tokio's LIFO slot, and it is exactly the "grantee runs next" placement.
- Malthusian locks (Dice, EuroSys '17): deliberately unfair admission for throughput.
- RCL (Lozi et al., ATC '12) and ffwd (Roghanchi et al., SOSP '17): delegation to a dedicated server core, which relates directly to the dropped actor control.
- ShflLock (Kashyap et al., SOSP '19): policy-by-shuffling, which the repo's own `cfl` lock implements (FINDINGS 2026-09-30 "async CFL", absent from the paper).
- Concord (SOSP '23): approximate preemption in cooperative scheduling, the obvious counterpoint to "a cooperative executor has no preemption to fall back on" (L262).

**Bottom line for §1.** The contribution that is both new and solid is narrow: a
measurement of combiner burden in a coroutine executor, and a two-part fix (bound plus
placement) that restores worker fairness for about 1–2 %. The fairness result is
u-SCL transplanted, and the speed result is unresolved (issue 1). That is a
workshop-sized or short-paper contribution unless issue 1 turns into a positive,
general result.

---

## 2. Does the evidence support each headline claim?

Every number below was checked against the FINDINGS top table and the co2 JSONs.

| Claim (line) | Source | Verdict |
|---|---|---|
| FIFO service Jain 0.65–0.67 at 1:8 (L60, L109) | Tab. 1 (09-28): 0.651–0.663; co2: dispatch 0.668, co-fifo 0.674, ces 0.668, fc 0.665, tokio 0.671 | Holds (sustained) |
| "…all reproduce their own prediction to within 0.001" (L71, L109) | Tab. 1 | True but **not a validation.** With L:H = 1.00 measured, each class's service is ops × CS, and the two-class J(x) is then an identity. The only empirical content is "FIFO gives equal ops", which is the definition of FIFO under saturation. Present it as an explanation, not a prediction |
| Delegation burden Jain 1/W, "which lock-side wake placement repairs" (L60) | 09-28 matrix; co2 burden 0.976–1.000 | Holds in the spin-bystander world. **Omitted:** REVIEW I4 sleep world. With parking workers, `fc-remote` bystander p99 is 238 µs and `fcpq-h16-home-c16` is 100 µs (G3 fails), and `home` wakes are the slowest variants (0.134–0.145 Mops/s). The recommendation inverts once workers can idle |
| co-pq service Jain ≥ 0.999 in all 10 cells, no starvation (L60, L77, L200) | co2: 10 label×cell combinations, 0.999–1.000 | Numerically true. Two caveats. (a) The 10 are 3 load cells × 2 modes plus c0 duplicates of 2 of them: c256 and c0 are the same lock with an unbinding clamp. (b) The 4 bursty cells are fair *because* co-pq is slow (next row) |
| "In bursty cells co-pq is fair (0.999) where the combining lock is not (0.702)" (L200) | co2 JSONs, my per-class computation | **Overclaim.** co-pq bursty: `fast = free = 0`, so every acquisition is contended, only because o = 15 566 keeps all 16 clients queued. Per-client ops/s, bursty spin: **light 12.5 k vs 24.6 k (FC-PQ) and 19.9 k (co-fifo); heavy 2.5 k vs 19.9 k and 18.2 k.** Every client of both classes is worse off under co-pq, and Jain calls that "fair". The old *inline* co-pq (co1) was 0.697 bursty, the same as FC-PQ, which confirms that fairness here tracks slowness. FC-PQ's bursty L:H of 1.23 is roughly demand-proportional, (32 000 + 8 000)/(32 000 + 1 000) ≈ 1.21 [INFERENCE: ignoring wait]. That is not a lock choosing favourites |
| co-pq o = 5771, 4.9× FC-PQ's 1183; 0.44× ops/s at L:H 5.25 vs 5.32 (L60, L202) | co2 | Numbers hold (5771/1183 = 4.88) |
| co-fifo 1200 "within 5 % of the ops/s of a combining lock (991)" (L60, L208) | 0.352/0.371 = 0.949 | 5.1 %. Write "0.95×" |
| Inline hand-off "4.5× lower cost than an ordinary-waker hand-off (dispatch, 5453)" (L60, L208) | 5453/1200 = 4.54 | Holds, *on this executor* |
| "Executor awareness, not the fairness policy, buys speed" (L60, L218, L267) | — | **Not supported beyond this executor** (issue 1). The same draft reports tokio's ordinary wake at o = 1340 under yield |
| tokio 1340 cycles/op under yield, 1.61× our dispatch (L60, L230) | 0.347/0.216 = 1.61; bursty 0.324/0.104 = 3.12 | Holds |
| tokio under yield "equals the locks that hand off inline" (L230) | Sustained 0.347 vs 0.351 / 0.362: yes. Bursty 0.324 vs 0.299 / 0.292: **1.08–1.11× faster** | Sustained OK; bursty wrong direction. Say "matches sustained, exceeds bursty" |
| Clamp binding rule; "predicted Jain 0.9395, measured 0.940" (L81, L152–154) | FINDINGS l.1417–1421 | **Circular.** "0.216 of requests promoted … heavy share of ops is 1/4.64 = 0.216. So the interval is (c + 1) + 0.28 = 9.28". The 0.28 "return delay" is back-solved from the measured L:H = 3.64, then fed back to "predict" L:H = 3.64 and Jain 0.9395. The part that really predicts is the binding threshold c ≤ 11, which is bracketed by a single pair (8 binds, 16 does not). Claim only that |
| dispatch-pq "needs 256 to reach Jain 1.000" (L224) | 09-29: clamps 16 and 256 only; co1 co-pq c64 binds (Jain 0.711) | "256 suffices; 64 and 16 bind". Nothing between 64 and 256 was measured. The draft's own [inference] says c ≳ 149–184 |
| co-pq c256 ≡ c0 in throughput and Jain; worst wait 257 vs 3102 (L224) | co2 | Holds. Also give it in µs: 257 × (C̄ + o) ≈ 257 × 8 460 / 2.2 GHz ≈ **1.0 ms** [computed], above the heavy p99 of 804 µs |
| Burden 0.976–1.000 in every sustained W8 cell; 0 starved in 132 runs (L192) | co2 | Holds. But see issue 8: for co-pq, `total_foreign_cs_cycles / cs_cycles` = 1254/1396 = **0.90**, so nearly every grantee's CS runs on the releaser's worker, and a Jain over per-worker sums cannot see it |
| Async release: sync 0.66× sustained, 0.19× bursty (spin); 0.98× / 0.91× (yield) (L212) | co1 (another binary, 09-29 23:17 UTC) | Numbers hold in co1. What the evidence shows is that an async release matters when the continuation computes synchronously and is worth 2–9 % when it awaits soon. L212's "that is what `unlock().await` buys" should carry that qualifier |
| Monopoly (async-lock 11/12 runs) and seizure (std/parking_lot) as forms of subversion (L71, L113) | 09-28, tokio runtime, **spin harness only** (Fig. 1b caption) | **Same artefact class as the withdrawn LIFO form.** REVIEW I1: std-mutex under yield has 0 starved clients (Jain 0.53, bystander p99 1.7 ms). The paper withdrew one spin-harness form (L117) and kept two others without re-measuring them |
| Explanation of the co-pq cost [inference] (L202) | — | Labelled as inference, which is good. A sharper explanation, testable with flags only, is given under issue 1 below |

### Issue 1 in detail: where the 5 771 cycles most likely go [INFERENCE, testable]

`co_mutex.rs:625-631`: the usage-ordered release calls `waker.wake_by_ref()` for the
grantee, then `cx.waker().wake_by_ref()` for itself, and returns `Pending`. The
executor's default placement (`executor.rs:422`) is `ctx.push_local`, i.e. the back
of the **releaser's** local FIFO. So G₁ (the grantee) and R₀ (the releaser) sit behind
the bystander on the releaser's worker. G₁ runs its CS and releases, and G₂ is queued
behind R₀. R₀ then runs its 4 000-cycle parallel work (spin), or yields first and
runs it one rotation later (yield); the bystander's poll also comes before G₂. The
lock is granted but idle through all of this.

The per-hand-off gap is therefore about one parallel-work quantum P plus one
bystander quantum t_B. From the JSONs:

| lock (W8 sus) | o | P | t_B (measured per bystander poll) | residual o − P − t_B | lock busy (CS / window) |
|---|---|---|---|---|---|
| co-pq spin | 5771 | 4000 | 1266 | ≈ 505 | 0.318 |
| co-pq yield | 5624 | 4000 | 1264 | ≈ 360 | 0.323 |
| dispatch spin (sync release, back of queue) | 5453 | 4000 | 1259 | ≈ 194 | 0.477 |
| tokio spin (LIFO slot, sync release) | 4419 | 4000 | — (next poll) | ≈ 419 | — |
| tokio yield (LIFO slot runs grantee before the releaser's work) | 1340 | 0 | — | — | — |
| co-fifo (inline run-next) | 1200 | 0 | 0 | — | 0.808 |

This fits all four ordinary-wake cells with no free parameter. It also explains why
yield does not help co-pq on this executor while it does help on tokio. It predicts
that the ordinary-waker cost is not "a scheduler round trip" but "the grantee waits
behind the releaser's own continuation". That is the very "where" decision the paper
says the lock makes, made here by the executor's `push_local`.

Supporting evidence already in the repo: the dispatch-pq-home hand-off breakdown in
FINDINGS "async CFL" (a): spin 223, queue 558, **grant→start 3 927** of 4 708 per
hand-off. That answers most of the L202 TODO. Cite it now.

**Test (flags only, 1–2 h):** `--parallel-work-cycles {0, 1000, 2000, 4000, 8000}`
× `--bystander-work-cycles {250, 1000, 4000}` (and `--bystanders 0`) for co-pq,
dispatch, co-fifo and FC-PQ, W8 sustained, spin and yield. Prediction: dO/dP ≈ 1 and
dO/dt_B ≈ 1 for co-pq and dispatch; ≈ 0 for co-fifo and FC-PQ. If that holds, the
paper has a one-line cost model to replace L202's [inference]. If it fails, the TODO
breakdown is the next thing to instrument.

---

## 3. Methodological gaps a PC would reject on

In order of how likely each is to be the stated reason for rejection.

1. **No real workload.** The CS is `insert + while rdtscp() < end`, and locality is
   excluded by construction (L100, which the draft admits). There is no application
   and no data-dependent CS cost. SOSP/OSDI expects at least one real service: for
   example a tokio-based KV/cache server with heterogeneous request costs, a
   connection-pool or buffer-pool mutex in a Rust DB, or a hyper/axum app with a
   shared-state lock. It should report end-to-end tail latency.
2. **The proposed locks never run on a real runtime.** Everything except
   `tokio::sync::Mutex` runs on a ~680-line custom executor whose wake policy
   (FIFO-back, no LIFO slot, a balancing steal invented for this workload) drives the
   headline cost. REVIEW I6 asked for a port and it was not done. This is a very small
   port for co-pq: its hot path uses only `Waker::wake_by_ref` plus `worker_id()` for
   the burden stats (`co_mutex.rs:626-629, 785, 854`).
3. **The bystander model dominates the machine.** In every co-pq and dispatch cell,
   always-runnable spinning bystanders take 0.80–0.82 of every worker's cycles, and
   0.62–0.78 in the others (JSON `bystander_poll_cycles / window`). They put a fixed
   t_B ≈ 1 265-cycle quantum into every FIFO-back hand-off, and they are the reason
   steal-when-idle never fires and balancing had to be invented (L94). REVIEW I4
   measured a sleep/park world in which the placement conclusions invert; the paper
   does not report it. A PC will see Q1–Q3 as properties of this bystander model.
4. **Closed loop only; no latency-vs-load.** Everything is at saturation (sustained)
   or a fixed bursty point. There are no arrivals, no open-loop latency curves, and
   no p99.9. In practice async mutexes are mostly uncontended, and the fast path was
   never taken in any measured cell (L239).
5. **Scale and machine.** W ≤ 16 on one socket of a 2 × 32-core machine, CPUs 0–15
   capped at 3.0 GHz for unexplained reasons (L182), one lock instance, 3 repeats with
   min/max, and no commit id (L180 TODO).
6. **Fairness metric.** Service Jain alone rewards outcomes that are worse for
   everyone (bursty, §2). Report per-class ops/s, or share relative to a FIFO
   reference, next to Jain. The sustained picture is then honest and still
   interesting. Against co-fifo, co-pq gives light clients 6.8 k/s vs 5.5 k/s
   (+24 %) and heavy clients 1.3 k/s vs 5.5 k/s (−76 %). FC-PQ dominates co-pq in
   both classes (15.6 k and 2.9 k).
7. **Burden metric blind spot.** Foreign-CS Jain is 1.000 for co-pq while 90 % of its
   CS cycles are foreign. The metric is a per-worker sum over 2 s and cannot tell
   "every worker equally" from "one worker at a time". FINDINGS (co1 (d)) already
   found the `-snone` one-worker collapse invisible to it. Add the per-worker
   client-poll share, over time windows, or the client-poll concentration statistic
   from `summarize_co.py`.
8. **Post-hoc parameters.** Clamp 256 / off, H = 16, and `home` vs `remote` were all
   chosen after looking at the data (L239, REVIEW I9.8). Pre-register the next window
   and separate exploratory findings from confirmatory ones in one table.
9. **Cumulative usage.** Without decay or slices, the policy favours returning
   clients: an intermittent client gets 1.0× the share of an always-present one
   (L237). The rescoping is honest. A systems PC will still read it as "the fair lock
   is not deployable", because clients always come and go.

---

## 4. Storyline and structure

**The promise and the delivery do not match.** The title and abstract promise a
design principle for "coroutine runtimes" (plural). The evaluation delivers:
- a u-SCL-style policy that is fair on any wake path, which is expected;
- a cost that, on one custom executor, tracks where the grantee is queued;
- one data point on tokio that points the other way.

The abstract's own clause "and the ordinary-wake cost depends on the runtime"
contradicts the title's "Executor Awareness Buys Speed".

### Recommended reframe (keeping the binding API decision)

The coroutine-style API and the ordinary-waker co-pq stay as they are (RESEARCH.md,
"API assumption"). Tell the story as **who / where**:

1. *Who.* The grant order fixes the share (§2 FIFO ∝ cost; usage order gives equal
   time). Keep it short: this is the expected part.
2. *Where.* The hand-off cost is set by where the grantee lands relative to the
   releaser's continuation. Model it (o ≈ P + t_B for FIFO-back, ≈ 0 for
   next-poll/inline) and validate it with the P/t_B sweep and on tokio (LIFO on and
   off). This is the part that can be new and general.
3. *Where, for delegation.* Combiner burden and the K-bound/home-break fix (Q1): the
   strongest existing result. Promote it to a first-class contribution and add the
   sleep-world result honestly.
4. *When.* The async unlock as the step-aside point. It matters for synchronous
   continuations and is worth ≤ 9 % otherwise.

### Cut

- "What we withdrew" (L241–247). Move it to a cover letter or changelog. Reviewers do
  not know the earlier version, and the section signals instability.
- Every reference to "an earlier version", "the review" or "adversarial review"
  (L77, L117, L230, L237, L241). They break double-blind review and read like a lab
  notebook.
- The "Three measurement windows" paragraph (L182) and the dates in captions
  (Fig. 1, 3, 4, 5; Tab. 1–3). Move provenance to an artifact appendix and keep one
  sentence in Setup.
- "Hand-off delay is not a form of subversion we can claim" (L117). If the form is
  not claimed, do not discuss it in §2.
- The newcomer-init options, `ces-t` (cycle budget), and fc-home details (L190). These
  are knobs a reader does not need.
- Red TODOs. Either run the item or state it once in Limitations.

### Move

- Q5 (tokio) into Q2/Q3. It is the control for the central claim, not an appendix
  question.
- Q4 (clamp) into §4.3 as a short design note plus Table 3.
- Q1 (burden) earlier, or into its own section, as a headline result.
- Tab. 1 into two sentences of text. It is an identity (§2).

### Contributions list (L80–82)

Contribution 3, "An evaluation from one frozen binary in one window", is
methodological hygiene, not a contribution. Replace it with the cost model and
cross-runtime result once they exist.

---

## 5. Writing: specific sentences to fix

| Line | Current | Problem | Suggested |
|---|---|---|---|
| L8 | "Ordinary Wakers Buy Fairness, Executor Awareness Buys Speed" | Wakers buy nothing: the grant order does. The second clause is untested on a real runtime | e.g. "Who and Where: Locks as Hidden Schedulers in Coroutine Runtimes" |
| L53 | "Draft --- `crates/coro_delegation`" | A repo path as the author line | Anonymous author block |
| L60 | "We show that a lock is therefore a hidden scheduler that can subvert the executor" | "therefore" does not follow from the preceding sentence | "Because it makes both decisions, a lock acts as a second scheduler that can override the executor's." |
| L60 | Seven numbers in four sentences (5771, 1183, 1200, 991, 5453, 1340, 1.61×) | The abstract cannot be read at a glance | At most three numbers: FIFO Jain 0.67 → 1.00; o 5.8 k vs 1.2 k; tokio 1.3 k |
| L60 | "Executor awareness, not the fairness policy, buys speed, and the ordinary-wake cost depends on the runtime" | The sentence contradicts itself | State the placement result directly |
| L60 | "The critical section is a TSC-timed spin, on one machine, with clients that never leave." | A limitation dropped at the end of the abstract | Move it to §7, or fold it in: "on a synthetic, closed-loop benchmark" |
| L67 | "Coroutines are back in wide use: many modern languages support cooperative multitasking~@ces." | Weak opener; cites a lock paper for a language-landscape claim | Open with the concrete problem (tokio mutex fairness, #6049) |
| L69 | "If unlock is an explicit `await` …, the lock can make the second decision without a callback into the executor." | Unclear what "the second decision" and "callback" mean | "An `unlock().await` lets the lock suspend the releaser once, so it can order the releaser after the grantee without any executor API." |
| L71 | "all reproduce their own prediction to within 0.001" | Presents an identity as a validation | "…as FIFO must: with equal operations per client, service is proportional to cost (J = 0.66)." |
| L77 | "This contradicts our expectation … It also qualifies an earlier version of this paper, which attributed…" | Narrates the authors' history; breaks double-blind | Delete. State the result |
| L82 | "An evaluation from one frozen binary in one window…" | Not a contribution | Replace (§4) |
| L102 | `$o=(T-C S)\/o p s$` and L94 `$l e n \/ W+1$` | Renders as italic 𝐶𝑆, 𝑜𝑝𝑠, 𝑙𝑒𝑛 | Use `"CS"`, `"ops"`, `"len"` upright |
| L119 | "hand out lock time in proportion to cost or to luck" | "luck" is unmeasured and informal | "…in proportion to cost, or to one client (monopoly)" |
| L124 | "We turn these requirements into goals" | "these requirements" has no antecedent: §2 lists forms | "Each form of §2 yields one goal." |
| L133 | "G1, G4 and G5 pull apart: what an ordinary waker costs is a property of the runtime that places it." | Asserted before any evidence; unclear what "pull apart" means | Drop it, or state the tension concretely after Q3 |
| L144 | "so the releaser re-queues behind whatever the executor runs first" | Not what happens in co-pq: the releaser queues behind *the grantee*, but the *previous* releaser sits ahead of the *next* grantee (issue 1) | Describe the queue order exactly; it is the mechanism of the result |
| L152 | "where 0.28 is the return delay (parallel work plus wake)" | Back-solved from the measured L:H (FINDINGS l.1418–1421) | "…0.28, fitted from the measured heavy share" and drop "predicts" at L154 and L81 |
| L154 | "A hand-off clamp must be about 16× looser than a 16-op-pass clamp; the default is 256." | "16×" is unexplained; 256 is post hoc | "A hand-off clamp must exceed the ~219-hand-off heavy round; we use 256 (chosen after a pilot)." |
| L173 | "`co-fifo` and `co-pq` were not probed [TODO…]" and "We have no TSan record for the other locks [TODO]" | **Stale.** FINDINGS l.410–422: TSan 9/9 with 0 reports on the co_mutex tests, and an allocation probe (lock allocates nothing; `remote` step-aside 0.0159/op from the Injector) | Cite them, with the caveat that they predate the ordinary-waker cutover of co-pq |
| L200 | "In bursty cells `co-pq` is fair (0.999) where the combining lock is not (0.702…)" | Pareto-dominated (§2) | "Bursty, co-pq's Jain is 0.999 only because its slow hand-off keeps all 16 clients queued; every client completes fewer operations than under FC-PQ." |
| L202 | "That contradicts what we expected of the ordinary wake plus async step-aside, which was to land near the executor-aware locks; it is _worse_ than…" | Narrates an expectation; italic emphasis | "co-pq is slower than dispatch-pq with home wakes (0.85× ops/s)." Then give the mechanism |
| L208 | "within 5 % of combining" | It is 5.1 % | "0.95× combining" |
| L230 | "Under yield it equals the locks that hand off inline" | Bursty tokio is 1.08–1.11× faster | "…matches them sustained and exceeds them by 8–11 % bursty" |
| L241 | "An adversarial review found that parts of an earlier version were artefacts." | Internal process | Delete the section (§4) |
| L258 | "CES states that it is the first delegation-style lock for cooperatively scheduled tasks; the claim is theirs." | Defensive and awkward | "CES introduced delegation for cooperatively scheduled tasks." |
| L267 | "Our hope is that runtimes will let a lock choose where its wakes run, so that fairness and speed no longer need to be traded." | tokio's ordinary wake already runs the grantee next; the paper's own Q5 says the trade may not exist there | Make it conditional on the tokio co-pq result |

**Global style points.**
- Prose is full of configuration labels (`fcpq-h16-home-c16`, `dispatch-pq-home-c256`, `co-fifo-sremote-k64-home`). Define short names once (FC-PQ, DPQ-home, co-fifo) and keep the labels in tables.
- Paragraph-leading bold run-ins with `#par(first-line-indent: 0pt)` are used for almost every paragraph in §2, §4 and §6, which flattens the hierarchy. Use them only for true run-in headings.
- Numbers density: most evaluation paragraphs carry 8–15 numbers. Put the numbers in the table and let the text give the one ratio that matters.

---

## 6. The single most important next experiment, and the verdict

### Next experiment: ordinary-waker co-pq on tokio

Run the unchanged `co-pq` (ordinary `Waker::wake_by_ref`, async step-aside, clamp
256; the binding API decision is untouched) inside `tokio-bench` on tokio's
multi-thread runtime, beside `tokio::sync::Mutex`, with the same keys, costs and
harness:
- W8 and W16; sustained and bursty; spin and yield; 3 repeats; one window.
- LIFO slot on (default) and off (`Builder::disable_lifo_slot`, behind `tokio_unstable`).

The port is small: co-pq's hot path only needs a stub for `worker_id()` (None →
`NO_WORKER`; burden stats off) (`co_mutex.rs:785, 854, 919`).

**What each outcome means:**
- o(co-pq, tokio, LIFO on, yield) ≈ 1.3–1.7 k at Jain ≥ 0.99: ordinary wakers give fairness *and* speed on the runtime people use. The title's second clause is wrong in general and right only for FIFO-back executors. The paper becomes "fair async locks are cheap iff the grantee runs next", a clean and positive systems result.
- ≈ 5 k with the LIFO slot on: the title stands on a real runtime, and the paper gains the external validity it lacks.
- LIFO off: gives the causal toggle that L230 admits was not run ("we did not toggle the slot in this window").

Run the flag-only P/t_B sweep on the coro executor alongside it (§2, issue 1) to turn
the L202 [inference] into a model.

Second priority, needed for any top-venue submission: one real application with
heterogeneous CS costs and end-to-end tail latency.

### Likely verdict

**OSDI/SOSP: Reject** (overall 2/5, reviewer confidence high).
- *Contribution:* a measurement study that transplants u-SCL-style usage ordering and CES-style inline hand-off into one custom executor. The newest result (burden plus bound/home break) is modest and is not the headline.
- *Central claim unresolved:* the "speed" half of the title is contradicted in spirit by the paper's own tokio data point and is plausibly explained by its executor's `push_local` plus bystander quantum. The "fairness" half is expected, and its breadth (10 cells) is partly an artefact.
- *Method:* a synthetic TSC-spin CS, spinning bystanders that use most of the CPU, closed loop, W ≤ 16, no application, no second runtime for the proposed locks.
- *Presentation:* 11 TODOs, a withdrawn-claims section, references to earlier versions and internal reviews, internal dates in every caption. As submitted it would be desk-flagged for double-blind issues.

**EuroSys: weak reject**, with the same reasons, softened by the careful measurement
hygiene: frozen binaries, same-window comparisons, spreads, and honest inference labels.

**What would move it to accept (EuroSys or ATC):**
1. co-pq on tokio, with LIFO on and off.
2. A validated placement cost model (the P/t_B sweep).
3. One real application with tail latency.
4. Decayed or sliced usage, with an intermittent-client scenario.
5. The reframe in §4, with Q1 promoted and the sleep-world result reported.

With (1)–(3) the paper has a general, falsifiable systems claim; without them it is
an informative lab report on one executor.
