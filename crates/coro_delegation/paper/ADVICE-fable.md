# PC review: "Fair Locks for Coroutine Runtimes: Ordinary Wakers Buy Fairness, Executor Awareness Buys Speed"

Reviewer role: senior PC, OSDI/SOSP/EuroSys. Source reviewed: `paper.typ` at
`d99b41f0` (line numbers below refer to it), `tables/numbers.typ`,
`FINDINGS.md` (top entry, `co2-*`), `REVIEW-2026-09-30.md`, `RESEARCH.md`,
`src/locks/co_mutex.rs`, `src/executor.rs`, and the `co2-*` JSONs. Everything
marked [INFERENCE] is my reasoning from code, not a measurement.

**Verdict: reject in current form; encourage resubmission after one reframing
and two experiments (§6).** The measurement hygiene is unusually honest. The
problem is that the paper's headline causal claim is contradicted by its own
Table 2, and the number the title rests on (`co-pq` at 4.9× FC-PQ's per-op
cost) is a property of the authors' executor plus a workload parameter, not of
ordinary wakers.

---

## 0. Ranked issues

| # | Severity | Issue |
|---|---|---|
| R1 | invalidates title/abstract | "Ordinary wakers" is not a cost class. `tokio::sync::Mutex` uses only ordinary wakers and costs o = 1 340 (yield); coro `dispatch`, also ordinary wakers, costs 5 453. The 4–5× is what *this executor* does with a same-worker wake (back of the local FIFO, no LIFO slot), not "executor awareness vs ordinary wakers". The paper says so itself at L133 and L230 and then keeps the title. |
| R2 | invalidates Q2/Q3 magnitude | The 4.9×/4.5× are ≈ (parallel-work P + admin)/admin: `o(co-pq) − o(co-fifo) ≈ 4 570 ≈ P = 4 000` plus a poll. The releaser's continuation is FIFO-ahead of the grantee-after-next on the same worker; `yield_now` does not remove it (it moves the spin to a separate poll in the *same* FIFO). L214's "the harness worry … does not explain the sustained ordinary-waker cost" is an incorrect inference. Change P and the headline ratio changes. |
| R3 | rejection-grade gap | The obvious baseline—usage-ordered lock with an inline grantee wake (chain bound 64, home break)—is missing on the current code, while the paper's own earlier data (`co1-*`, FINDINGS) has it at o = 1 519, Jain 0.999, 0.87× FC-PQ ops/s. No lock of the paper runs on tokio. G5 "executor independence" is asserted. |
| R4 | weakens contribution | Novelty is thin relative to SCL/u-SCL (usage order + clamp; SCL §7 already proposes it for delegation), CES (inline resume + remote reschedule), FC/TCLocks (bounded combining rounds). What is new and supportable is small: (i) the fair-policy increment is ≈ 200–300 cycles/op regardless of mechanism; (ii) chain bound + *home* break repairs combiner burden; (iii) the clamp-unit rule. Lead with those. |
| R5 | weakens every generalisation | Workload artefacts are load-bearing: TSC-spin CS (locality excluded), always-runnable spinning bystanders + a balancing steal invented to cope with them, closed-loop always-present clients (cumulative usage rewards absence, admitted), no timer/sleep. "Monopoly and seizure" (L71, L113) were measured only under the spin harness; the review showed std-mutex whole-run seizure vanishes with a yield; `async-lock` was not rerun. Calling them "structural" is an overclaim. |
| R6 | methodological | Three clock windows with a mid-study frequency cap; no commit id; 3 repeats with min/max; 6 % histogram buckets; bursty `o` includes idle; two incompatible burden definitions; `o` never decomposed ("we have not broken o down", L202). |
| R7 | structure | Draft artefacts (red TODOs, "Draft" byline, "What we withdrew" list, window dates like "09-28") and the crucial hedge buried in Q5. Q1 (burden) is the strongest self-contained result and is under-sold. |
| R8 | writing | Math-mode identifiers rendered as products (`l e n`, `C S`, `o p s`), contradictions between L97 and L230/L244 on whether `dispatch` is tokio's mutex, narrated surprise ("This contradicts our expectation") in place of mechanism. |

---

## 1. Contribution, thesis, novelty

**Stated thesis (one sentence, as I read it):** *A lock in a coroutine runtime
is a hidden scheduler; a usage-ordered grant order restores service fairness
with nothing but ordinary wakers, while speed requires executor-aware wake
placement (inline hand-off or combining).*

**Is it crisp?** Yes. **Is it supported?** Half. The first clause is supported
(Table 2: `co-pq` Jain 0.999–1.000 in every cell) but it is also close to a
tautology—the successor choice is made under the lock's spinlock before any
wake, so no wake mechanism can change *who* is granted; only a wake that is
*lost* could. The second clause is not supported, because "ordinary waker" is
not a mechanism: it hands placement to the runtime. On tokio the ordinary
waker issued from a worker lands in the LIFO slot (an inline hand-off, capped
at 3) and the FIFO mutex costs 1 340 cycles/op under yield; on this executor it
lands at the back of the waking worker's local FIFO and costs 5 453 (`dispatch`)
to 5 771 (`co-pq`). Same waker API, 4× apart, inside one table. The correct
thesis the data support is:

> *The fair (usage-ordered) grant order costs ≈ 200–300 cycles per operation
> on top of any hand-off mechanism; the 4–5× spread between hand-off locks in a
> cooperative executor comes from where the runtime places the grantee relative
> to the releaser's continuation, not from fairness or delegation.*

Evidence for the increment, all same-placement pairs, one window:
`co-pq − dispatch` = 5 771 − 5 453 = 318 (includes one extra step-aside poll);
`fcpq-h16-home-c16 − fc-remote` = 1 183 − 991 = 192;
`co1` inline `co-pq − co-fifo` = 1 519 − 1 208 = 311 (earlier window, ratio only);
review's `dpq-inline − dispatch-inline` (yield) = 1 660 − 1 112 = 548.
That is a crisp, quantitative, and genuinely useful statement. It is not what
the title says.

**Novelty relative to the named prior work.**

- *SCL / u-SCL (EuroSys'20).* u-SCL is exactly a usage-ordered, non-delegating
  lock with a starvation bound; SCL §7 already suggests usage-fair scheduling
  of delegated requests. `co-pq`/`dispatch-pq` are u-SCL in async clothing
  minus lock slices (the paper says so, L254). The fairness policy is not a
  contribution; its *increment* measured against matched mechanisms is.
- *CES.* Inline resume of the head waiter and remote rescheduling of the owner
  are CES's. The paper adds a chain bound with a *home* break and shows that
  the bound alone does nothing at b0 while bound + home break gives burden Jain
  0.99+ at ≤ 2 % throughput cost (Q1). That is a real, if small, mechanism
  contribution. The explicit `unlock().await` is a nice API point, but its
  measured benefit (Q3 "async release") is only against `drop(g)` under the
  spin harness, i.e. against review issue I1.
- *FC / TCLocks.* Bounded combining rounds and remote/home wake placement are
  parameters, not designs. The per-worker burden metric and the observation
  that `fc-home` re-converges (0.744) are new measurements.
- *tokio.* The paper's most important datum about tokio—its ordinary waker *is*
  an inline hand-off—appears only as a Q5 hedge. tokio's issue tracker already
  documents the LIFO slot and the mutex's waiter-only fairness.

**What would be a contribution reviewers would accept:** the "hidden scheduler
with two decisions (who, and when/where the releaser steps aside)" framing, the
placement contract as a *named* design space (inline / remote / home / default)
with the burden and cost consequences of each measured, the clamp-unit rule,
and the fair-policy increment. That paper exists inside this one.

## 2. Evidence check

Checked against `numbers.typ` / FINDINGS `co2` table / the r1 JSONs.

| Claim (line) | Numbers | Status |
|---|---|---|
| Abstract L60: FIFO service Jain 0.65–0.67 at 1:8 | 0.665–0.677 across FIFO locks | OK |
| L60: `co-pq` Jain ≥ 0.999 in all 10 cells, no starvation | 0.999–1.000; starved 0/0 | OK, but "10 cells" counts clamp 256 and 0 of the same lock separately: 6 configurations |
| L60: `co-pq` o = 5 771 vs FC-PQ 1 183, 4.9× | 5771/1183 = 4.88 | arithmetic OK; attribution wrong (§3) |
| L60: `co-fifo` within 5 % of combining ops/s; 4.5× lower o than `dispatch` | 0.352/0.371 = 0.949; 5453/1200 = 4.54 | OK (5.1 %, say "5 %") |
| L60: tokio 1 340 under yield, 1.61× `dispatch` ops/s | 0.347/0.216 = 1.61 | OK; this row contradicts the title (R1) |
| L71: predicted Jain 0.661 from 1 391/8 403 | x = 0.1655 → J = 0.661 | OK |
| L109: "predicts to three digits" | equal ops ⇒ service ∝ cost; J follows by algebra | true but trivial; not a finding |
| L113: `async-lock` monopoly 11/12; std/parking_lot seize W workers "for the whole run" | 09-28, spin harness only | **overclaim**: review I1 showed std seizure is a never-yielding-loop artefact (0 starved with a yield); `async-lock` not rerun |
| L115: CES burden Jain exactly 1/W; chain 819 200 | 09-28 data, not in `co2` | not checkable here |
| L152–154: clamp rule, c ≤ 11 binds, c ≥ 9 for J ≥ 0.95, 219 hand-offs | (c+1)+0.28 < 13.0; L:H 3.85 ⇒ c ≥ 8.4; 32 + 32·5.84 = 219 | OK |
| L154: J ≥ 0.95 needs x ≥ 0.627, L:H ≥ 3.85 | solve 0.9x² − 2x + 0.9 = 0 → 0.627; ×6.14 | OK |
| L190: `ces-k64-home` 0.98–0.99× CES | 09-28 | not checkable here |
| L192: burden 0.976–1.000, 132 runs, 0 starved | matches | OK |
| L200: bursty FC-PQ unfair (0.702) because H ≥ waiters | 7.8 ops/pass, 16 clients | OK, correct explanation |
| L202: co-pq 0.44× FC-PQ ops/s, 0.85× dpq | 0.260/0.592, 0.260/0.306 | OK |
| L202: "Our explanation [inference]…" | none measured | **unsupported and incomplete** (§3) |
| L208: co-fifo 1.67× dispatch, 0.98× CES, 0.95× FC | 0.352/0.211, /0.360, /0.371 | OK |
| L212: sync release 0.66× / 0.19× (co1) | 0.233/0.352; 0.059/0.305 | OK, but this is I1 by construction: `drop(g)` then spin P in the same poll |
| L214: yield moves no stepping lock > 3 %; "does not explain the sustained ordinary-waker cost" | ratios OK | **incorrect inference** (§3) |
| L224: clamp 256 vs 0: 257 vs 3 102 worst wait | matches | OK |
| L230: tokio 1.48× / 4.49× yield over spin | 0.347/0.233; 0.324/0.072 | OK |
| L237: intermittent heavy client 1.00× | review side experiment, n = 2 | honestly scoped, but it means G1 as stated at L127 is not met by the *design*, only by the harness population |

**Unsupported inferences to remove or measure:** L202 (o breakdown), L214
(yield excludes the harness artefact), L230 ("tokio's LIFO slot … we did not
toggle the slot"), L260 ("[inference from its source]"—read the source; it is
one function), L71/L113 "structural" forms.

## 3. Why `co-pq` costs 4.9× FC-PQ, and whether the paper's explanation is right

### 3.1 The wake/steal/poll path (from code)

`co_mutex.rs::place_grantee` (L624–634) for `USAGE_ORDERED`: after `release()`
has popped the minimum-usage waiter under the spinlock, granted, and dropped
the spinlock, it calls `waker.wake_by_ref()` on the grantee and then
`cx.waker().wake_by_ref()` on itself and returns `Pending`.

`executor.rs::schedule` (L406–430): hint is `Default`; on a worker,
`Default` ⇒ `push_local` ⇒ **back of this worker's `crossbeam_deque::Worker`
FIFO**. The grantee is not running, so its schedule runs synchronously and it
lands at the back of the *releaser's* worker queue. The releaser's self-wake is
deferred by `async-task` until its poll returns `Pending`, then also pushed
to the back—**behind the grantee**. There is no LIFO slot for `Default`.

`worker_main` (L693–751): run-next slot (empty for co-pq) → injector every 31
polls (empty: nothing is `Remote`) → balancing steal every 31 polls (half of a
random peer's queue) → **local FIFO pop**.

### 3.2 Steady state on the hot worker [INFERENCE from the loop above]

Every wake is issued by the current owner from the worker that polled it, and
the owner was polled where it was queued; so the whole grant chain sits on
*one* worker w until a balancing steal moves it. Trace w's FIFO (B = the
worker's bystander, 1 000-cycle spin then `yield_now` = push to back; Gₙ′ =
Gₙ's continuation after the step-aside):

```
[B, G1, G0′]         pop B (1 264)        → [G1, G0′, B]
pop G1: CS, unlock → push G2, then G1′    → [G0′, B, G2, G1′]
pop G0′: spin P=4 000, lock() → Pending   → [B, G2, G1′]
pop B                                      → [G2, G1′, B]
pop G2: CS …                               → [G1′, B, G3, G2′]
```

Between consecutive grantee polls the FIFO always contains exactly one
previous-owner continuation (P + enqueue) and one bystander poll. So

    o(co-pq) ≈ P + B + admin(≈2 polls, spinlock, heap pop, O(n) clamp scan, wake)
             ≈ 4 000 + 1 264 + ~500 ≈ 5.8k   (measured 5 771–5 804)

The match is closer than it should be: balancing steals (≈ 22 k per worker per
2 s, JSON `balance_steals`) sometimes move the continuation away, which is
also why W = 16 does not help (5 953: the chain is still on one worker at a
time) and why bursty `o` (15.6k) is about half of P = 32 000 rather than all
of it. The `dispatch` row is the same mechanism with the spin *inside* the
owner's poll: `o = 5 453 ≈ P + 1.2k admin`, and its bursty spin `o = 26 590 ≈
P = 32 000` minus stolen share, as FINDINGS already notes.

**Yield mode does not change this.** `co-pq`'s `yield_now` is another
`Default` push to the back of the *same* FIFO (3 polls/op in the JSON vs 2.0
under spin). The 4 000-cycle spin moves from Gₙ′'s first poll to its second,
but both polls are FIFO-ahead of Gₙ₊₂. Hence 5 624 vs 5 771. L214's conclusion
that yield rules out the harness artefact is therefore wrong: the artefact is
still there, one hop later. The right knob is `--parallel-work-cycles`, not
`--parallel-mode`.

Corroboration from the JSONs (`co2-*-w8-h8-b31-sus-spin-r1`):

| lock | window/op | CS/op | o | client-poll cycles/op | reading |
|---|---|---|---|---|---|
| `co-pq` | 8 461 | 2 685 | 5 776 | 8 744 (2.0 polls) | continuation serialised with the lock chain |
| `dispatch` | 10 441 | 4 975 | 5 466 | 10 214 (1.0 poll) | spin inside the owner's poll |
| `co-fifo` | 6 247 | 5 041 | 1 206 | 11 450 (1.98 polls) | poll cycles ≫ window/op: continuations run on *other* workers (Remote step-aside), o = admin only |

Bystanders take 81 % of all worker cycles in the `co-pq` run; the lock chain
is the bottleneck on one worker while seven others spin bystanders.

### 3.3 Is the paper's explanation right?

Partly. L202 says the grantee "joins the back of the releaser's local queue
behind the bystanders and other clients, and the releaser's own step-aside
adds a second queue pass". Correct: back of the local queue. Missing: (a) the
dominant term is the *previous* owner's continuation (P), which is also the
harness's parallel work, i.e. the I1 mechanism; (b) the bystander is second
order (`dispatch`'s o ≈ P + admin suggests B is often not ahead); (c) the
step-aside as coded is ordering-neutral for `co-pq`—the grantee is already
ahead of the releaser in the same FIFO—so it can only cost (one extra poll,
≈ 300 cycles, ≈ `co-pq − dispatch`). A `co-pq --co-step-aside none` cell is
missing from `co2` and would show this directly.

### 3.4 Inherent to ordinary wakers, or an artefact of this executor?

**Artefact of this executor's `Default` placement, interacting with a
closed-loop workload whose CPU-bound continuation shares the FIFO.** Four
independent facts, three from the paper's own data:

1. tokio's `Waker::wake` from a worker goes to the LIFO slot (inline, cap 3);
   tokio's `yield_now` defers the yielder to the maintenance tick, so neither
   the releaser's continuation nor the bystander is ahead of the grantee.
   `tokio-mutex` yield: o = 1 340. Ordinary wakers, 1.1× `co-fifo`.
2. Review side experiment: `dispatch-inline` under yield, o = 1 112; fair
   `dpq-inline-c256`, o = 1 660 at Jain 1.000. Same lock code, one placement
   flag.
3. `co1-*` inline `co-pq` (grantee in run-next slot, remote step-aside):
   o = 1 519, Jain 0.999, 0.87× FC-PQ ops/s.
4. The remaining gap after removing P—`co-fifo` 1 200 vs `fc-remote` 991,
   inline `co-pq` 1 519 vs FC-PQ 1 158—is 1.2–1.3×, which is the honest
   "cost of hand-off vs combining" in this setting.

What *is* inherent to an ordinary waker: it cannot express "run before the
releaser's continuation", so it is exactly as fast as the runtime's default
same-worker wake. On tokio that default is inline; on this executor it is the
worst of the three placements the paper defines. The paper's closing wish
(L267) that runtimes "let a lock choose where its wakes run" is misdirected:
tokio already gets the speed with no lock-side placement; what the paper's
executor lacks is a sane default.

**Testable predictions (cheap, same binary):** (i) `o(co-pq)` and `o(dispatch)`
are linear in `--parallel-work-cycles` with slope ≈ 1 (spin *and* yield);
`co-fifo`, `ces-k64-home`, `fcpq`, tokio-yield have slope ≈ 0. At P = 400 the
"4.9×" becomes ≈ 1.5×. (ii) `--bystanders 0` lowers `o(co-pq)` by ≤ 1.2k.
(iii) `--balance-interval 0` raises it toward P + B + admin and drives burden
Jain to 1/W. (iv) `co-pq --co-step-aside none` ≈ `dispatch-pq` default ≈
`co-pq` − 300.

## 4. Methodological gaps a reviewer rejects on

1. **One executor, authored by the authors, whose default placement drives
   every cost headline.** No lock of the paper runs on tokio; G5 and
   "lock-agnostic" are untested (review I6). The paper concedes at L230 that
   the cost "may be a property of our executor" and titles itself otherwise.
2. **The headline ratio is a workload parameter** (§3.2). The paper must report
   `o` as a function of P or at a P chosen from a real trace, and must state
   that the coro `yield` mode is not tokio's `yield_now` (defer list vs back of
   FIFO; the tokio baseline's own doc comment says so, `workload.rs:19–22`).
3. **TSC-spin critical section.** Locality, the standard argument for
   delegation, is excluded by construction (stated, L100, good) but no real CS
   variant exists (L235 TODO), so the direction of the term is unknown.
4. **Closed-loop, always-present clients with cumulative usage.** The review's
   intermittent experiment shows a client present 20 % of the time gets 1.00×
   the lock time of a steady one. G1 (L127) is defined for continuously
   contending clients only; SCL's slices exist for this. A decayed or windowed
   counter is a two-line change and must be measured before any fairness claim
   is made for a runtime whose whole point is tasks that come and go (I/O).
5. **Load-bearing bystanders and balancing.** Always-runnable spinning
   bystanders make steal-when-idle never fire; the balancing steal exists to
   compensate; no timer, no sleep mode. The review's sleep-world experiment
   inverted the placement recommendations (`home` worst when the home worker
   parks). Q1's conclusion holds "for this executor and workload" (L192) and
   that is the whole of Q1.
6. **Missing baselines:** usage-ordered inline hand-off on current code
   (paper's own TODO at L218; data exist in `co1`); `co-pq` with
   `StepAside::None`; `dispatch-pq` clamps 150–220 (the predicted Jain
   frontier, L239 TODO); `co-pq` on tokio; a tokio-native usage-ordered
   semaphore.
7. **Statistics and provenance.** 3 repeats with min/max; 2 s windows; three
   clock windows with a 3.0 GHz cap introduced mid-study (a 19 % change in
   non-spin time, attributed as "[inference]"); no commit id (L180 TODO);
   latencies at 6 % bucket resolution with "within one bucket" comparisons;
   bursty `o` includes idle (stated) and is still tabulated beside sustained
   `o`; burden Jain over two different quantities (stated).
8. **"Monopoly and seizure" measured only under the spin harness** and still
   called structural (L71). Rerun `async-lock`, `std`, `parking_lot` under
   yield or drop the form.
9. **No tail latency under load, no open-loop arrivals, no application, one
   lock instance, one socket.** Standard asks at this venue; the paper has none.

## 5. Storyline and structure

**Promise vs delivery.** Title and abstract promise a general law about
ordinary wakers versus executor awareness. The evaluation delivers: (a) a
fairness policy that works under any wake (expected), (b) a 4–5× cost spread
that is P/admin on one executor, (c) tokio's ordinary waker not showing the
spread. Readers who reach Q5 feel the title was written before Q5 was run.

**Reframe.** Lead with the two decisions (who; when/where the releaser steps
aside), the placement design space, and the *increment* of fairness
(≈ 200–300 cycles) versus the *cost of placement* (≈ P when the grantee is
FIFO-behind the releaser's continuation). Make tokio's LIFO slot the running
example of a runtime whose default is already the right placement, and this
executor the example of one whose default is not. Then Q1 (burden) becomes the
second pillar: the only subversion specific to delegation and the one the lock
can repair with a home break.

**Cut.**
- The "What we withdrew" list (L241–247): move to a cover letter or artefact
  appendix. In the paper it reads as instability, not rigour.
- Red TODOs and "Draft" byline (L6, L53, L173, L180, L218, L235–239). Either
  do the item or state the limitation once in §7.
- Window dates ("09-28", "09-29", "09-30") everywhere. Rerun the 09-28 cells
  (Fig. 1, Tab. 1, Fig. 2, Fig. 4a) in the frozen-binary window and delete the
  three-windows paragraph (L182). If that is infeasible, name them Window A/B
  once and never cite a cross-window ratio (the paper already promises this).
- "FIFO Jain predicted to three digits" as contribution 1 (L80). It is
  algebra; keep one sentence in §2.
- The `StepAside::Home` variant and the `co-pq -c0` "cells" (fold into text).
- L117 (hand-off delay is not a form we can claim): one clause in §7.

**Move.**
- L230's "the cost of `co-pq` may be a property of our executor" → abstract and
  introduction, as the finding, with the tokio row as evidence.
- L133 ("what an ordinary waker costs is a property of the runtime that
  places it") → the thesis sentence.
- Q5 → merge into Q2/Q3 as the cross-runtime control; Q1 → first evaluation
  question or its own section; §5.1's clamp-unit rule → keep, compress by
  half, it is correct and useful.
- The `co1` inline `co-pq` numbers → Q3, as ratios within their window, with
  the sentence "an inline usage-ordered hand-off costs 1.3× FC-PQ at the same
  fairness" (or rerun on current code, better).

## 6. Writing: specific fixes (quoted, with line numbers)

- L8 title: two claims; the second is unsupported (R1). Suggest: *"Fair Locks
  for Coroutine Runtimes: Fairness Is a Grant Order, Speed Is a Wake
  Placement."*
- L60 "It is not cheap." → state the mechanism: "On an executor whose default
  wake is the back of the waking worker's queue, the grantee waits behind the
  previous owner's continuation (≈ P cycles)…". Also L60 "Executor awareness,
  not the fairness policy, buys speed, and the ordinary-wake cost depends on
  the runtime" — the second half refutes the first; keep the second.
- L67 "Coroutines are back in wide use: many modern languages support
  cooperative multitasking~@ces." Weak opener, wrong citation for the claim.
  Open with the two decisions.
- L69 and L119 both define "executor subversion"; define once.
- L77 "This contradicts our expectation … It also qualifies an earlier version
  of this paper…" Remove self-narration; papers report mechanisms, not
  surprise or version history.
- L94 `$l e n \/ W+1$` renders as l·e·n; use `$"len"\/W+1$`. L102
  `$o=(T-C S)\/o p s$` renders C·S and o·p·s; use `$o=(T-"CS")\/"ops"$`.
- L97 "Under _dispatch_ (`dispatch`; tokio, Kotlin, Boost)" contradicts L230
  "our `dispatch` is not tokio's mutex" and L244. Remove tokio from L97 or
  say "tokio's *placement* differs (LIFO slot)".
- L100 "in `yield` the client first calls `yield_now().await`, as code that
  awaits I/O after unlocking would" — false for this executor: an I/O await
  removes the task from every queue; `yield_now` here pushes it to the back of
  the same FIFO. Say what it is.
- L133 "G1, G4 and G5 pull apart: what an ordinary waker costs is a property
  of the runtime that places it." This is the paper's true thesis; promote it.
- L144 "`co-pq` … wakes it with a plain `wake_by_ref()`: no placement hint, no
  inline hand-off … The runtime chooses where everything runs." Add: "on this
  executor that is the back of the waking worker's FIFO; on tokio it is the
  LIFO slot".
- L182 "later windows are hollow where one axis shows several" — unparseable
  without the figure legend; delete with the windows.
- L202 "We have not broken $o$ down. Our explanation [inference]…" The paper's
  central number must not be an inference (§3 gives the decomposition and the
  sweep that tests it).
- L214 "The review's harness worry … therefore explains … but not the
  sustained ordinary-waker cost of Q2." Incorrect (§3.2). Replace with the P
  sweep result.
- L218 "Fairness is a policy … Speed is a mechanism" — slogan; say
  "speed is where the grantee sits relative to the releaser's continuation".
- L230 "[inference from the spin/yield contrast; we did not toggle the slot]"
  — toggling `disable_lifo_slot()` is one builder call; do it.
- L242 "the hand-off cost is a property of the wake (ordinary against
  inline)" — of the *placement*, not the wake; the tokio row is an ordinary
  wake placed inline.
- L260 "[inference from its source, consistent with §@sec:q5]" — it is not an
  inference if you read the source; cite `tokio/src/runtime/scheduler/
  multi_thread/worker.rs` (`schedule_local`, `MAX_LIFO_POLLS_PER_TICK`).
- L267 "Our hope is that runtimes will let a lock choose where its wakes run"
  — tokio's data show a good default suffices; the ask should be "a capped
  wake-next default (tokio's LIFO slot) and an explicit step-aside".
- Labels such as `co-pq-sremote-k64-c256`, `fcpq-h16-home-c16` are CLI grammar;
  give each lock a one-word name in a table and use it.

## 7. The single most important next experiment, and the verdict

**Experiment.** Port `co-pq` (and `dispatch-pq`) to tokio—they need only
`Waker::wake`—and run the same W8 sustained/bursty × spin/yield matrix on both
runtimes in one window, alongside `tokio::sync::Mutex`, and on the coro
executor add `co-pq` with an inline grantee wake (chain bound 64, home break)
and `--co-step-aside none`. Predicted outcome [INFERENCE]: fair `co-pq` on
tokio under yield lands at o ≈ 1.5–1.9k (Jain ≥ 0.99), within 1.3–1.5× of
FC-PQ; the title's dichotomy dissolves and the paper's real result (fairness
costs ≈ 300 cycles; placement costs ≈ P) stands on two runtimes. As a cheap
same-day corroboration, sweep `--parallel-work-cycles ∈ {400, 1000, 4000,
16000}` for `co-pq`, `dispatch`, `co-fifo`, `fcpq` and plot `o` against P: two
lines with slope ≈ 1, two with slope ≈ 0.

Second priority: a decayed/windowed usage counter with an intermittent-client
scenario (G1 for the population async runtimes actually have). Third: a real
memory-bound CS variant.

**Verdict: Reject (resubmit encouraged).** Reasons: the title/abstract claim
is contradicted by the paper's own Table 2 (R1); the headline 4.9× is the
harness's parallel-work parameter re-entering through the executor's default
placement, and the paper's stated test for that (spin vs yield) does not test
it (R2); the natural baseline that the authors themselves identify and
previously measured is absent from the frozen-binary window (R3); nothing runs
on a second executor (R3). What would make me argue for acceptance next round:
the reframed thesis of §1, the two runtimes, the P sweep, the inline fair
hand-off, and one of {decayed usage, real CS}. The measurement discipline
(one binary, one window, stated mix, `o` instead of ops/s, [inference] tags,
withdrawn claims) is exemplary and should be kept—just out of the main text.
