#import "lib.typ": rng, fit
#import "tables/numbers.typ": *
#import "figures/contract.typ": contract

// A number or result the text wants but no FINDINGS.md entry or JSON provides.
#let todo(body) = text(fill: red, weight: "bold")[\[TODO: #body\]]

#let title = [Scheduler Subversion by Delegation Locks in Cooperative Executors:\ Combiner Burden, Service Unfairness, and Lock-Side Policies]

#set document(title: title)
#set page(paper: "us-letter", margin: 0.72in, columns: 2, numbering: "1")
#set columns(gutter: 0.25in)
#set text(font: "New Computer Modern", size: 10pt)
#set par(justify: true, first-line-indent: 1em, spacing: 0.65em, leading: 0.55em)
#show raw: set text(font: "Latin Modern Mono", size: 1.25em)
#set heading(numbering: "1.1", supplement: none)
#show heading: set block(above: 1.2em, below: 0.7em)
#show heading.where(level: 1): set text(size: 12pt)
#show heading.where(level: 2): set text(size: 11pt)
#set list(indent: 0pt, body-indent: 0.5em)
#set enum(indent: 0pt, body-indent: 0.5em)
#set math.equation(numbering: none)
#show figure.caption: set align(left)
#show figure: set block(breakable: false)

// A generated table in a float; `wide` spans both columns (LaTeX table*).
// `placement: none` keeps it in the text flow.
#let tbl(file, caption, wide: false, colsep: 6pt, placement: top) = figure(
  kind: table,
  placement: placement,
  scope: if wide { "parent" } else { "column" },
  caption: caption,
  {
    set text(size: 8pt)
    set par(justify: false)
    set table(inset: (x: colsep, y: 1.6pt))
    fit(include file)
  },
)

// A generated SVG figure (scripts/make_figures.py) in a float, included at
// the width it was drawn for, so its 7.5-8 pt text prints at size.
#let fig(file, caption, wide: false) = figure(
  placement: top,
  scope: if wide { "parent" } else { "column" },
  caption: caption,
  image(file, width: if wide { 7in } else { 3.3in }),
)

#place(top + center, float: true, scope: "parent", clearance: 2em)[
  #text(size: 17pt)[#title]
  #v(1em)
  #text(size: 12pt)[Draft --- `crates/coro_delegation`]
  #v(1.2em)
  #block(width: 85%)[
    #set text(size: 9pt)
    #set par(first-line-indent: 0pt)
    #align(center)[*Abstract*]
    #v(0.3em)
    Delegation (combining) locks execute other tasks' critical sections on one thread. In a cooperatively scheduled, work-stealing coroutine executor this subverts the scheduler in two ways. First, _combiner burden_: the worker that becomes combiner pays for everyone's critical sections, and so does every task queued behind it. In our measurements, a Combine-and-Exchange (CES) lock keeps one worker combining for the whole 2 s window (burden Jain index $1\/W$), and, with executor balancing disabled, flat combining (FC) without a cooperative yield lets one of 64 clients perform every operation. Second, _service unfairness_: FIFO hand-off gives each client service in proportion to its critical-section cost; with a 1:8 cost mix every FIFO lock we measured, `tokio::sync::Mutex` included, has the service Jain index predicted by that proportionality (0.65--0.66 sustained). We treat _where the combiner runs_ and _whom it serves next_ as lock policies that need only wake-placement hints from the executor. A 64-hand-off chain bound with a home-worker chain break (CES) and a pass-end hand-off with remote wakes (FC) restore burden Jain 0.98--1.00 at 0.95--1.01× CES throughput. Usage-ordered FC with pass limit 16, home wakes and a 16-pass starvation clamp reaches service Jain 0.997 with a worst queue wait of 17 passes. Under sustained load the two burden-fair locks run #(SusOnLo)--#(SusOnHi)× `tokio::sync::Mutex` (tokio 1.53.1), and #(SusOffLo)--#(SusOffHi)× with tokio's LIFO slot disabled. A per-lock server task (actor) reaches either delegation throughput or burden fairness, not both. Fairness costs lock utilisation: the lock's per-operation overhead of about 1 150 cycles is paid by cheap operations too, so utilisation falls from 0.84 to 0.68.
  ]
]

// ---------------------------------------------------------------------------
= Introduction <sec:intro>

Coroutine runtimes such as tokio~@tokio multiplex many tasks onto a few worker threads. Scheduling is cooperative: a task runs until it returns `Pending`. The runtime balances load by work stealing. Its fairness guarantees are weak and are stated only for runnable tasks. Locks for such runtimes are _async mutexes_: a waiter suspends, and the unlocker wakes it through the scheduler. CES~@ces observed that this hand-off puts a scheduler round trip on the critical path. CES instead suspends the unlocker and resumes the next waiter inline on the unlocking thread, so contended critical sections stay on one thread. This is a delegation lock in the sense of flat combining (FC)~@fc and TCLocks~@tclocks. CES reports throughput only.

Patel et al.~@scl showed that ordinary locks _subvert_ the OS scheduler: lock hold time, not the scheduler's share, decides who gets CPU time. We find two analogous subversions when delegation locks meet a cooperative executor:

+ *Combiner burden.* Critical sections run on whichever worker is combining, and the executor cannot see that. With CES the unlocking worker stays combiner for as long as the wait queue is non-empty. Under sustained contention that is the entire 2 s run: the burden Jain index over workers is exactly $1\/W$ in every saturated sustained cell at $W=4,8,16$, and a bystander task queued on that worker is never polled (@fig:motiv a, b). Without executor balancing, FC without a cooperative yield is worse. The combining task never returns `Pending`, so 63 of 64 clients do no work for 2 s.
+ *Service unfairness.* FIFO hand-off gives equal operations per client, so lock time is proportional to critical-section cost (@fig:motiv c). For a 1:8 light:heavy cost mix, the resulting service Jain index is 0.653 (from the measured 1 306:8 324 cycles per op). `dispatch`, CES, FC and `tokio::sync::Mutex` all reproduce it to within 0.01 (@tab:fifo).

#par(first-line-indent: (amount: 1em, all: true))[Our position is that both are lock policies, not executor policies. A delegation lock decides _where the combiner runs_ and _whom it serves next_. To act on the first decision, the only thing it needs from the executor is a placement argument on wakes (inline, remote, home). We make three contributions:]

- Measurements of both subversions on a work-stealing executor built for this study (§@sec:motiv). They cover `dispatch`, CES, FC, a usage-ordered FC (FC-PQ), and a cross-check against the real tokio locks.
- A minimal executor contract (`wake_with(placement)`, `reschedule_self_remote`, a run-next slot), and lock policies on top of it (§@sec:design). For burden: a CES chain bound with home-worker chain break, and FC yield-after-combine with remote wakes. For service: usage-ordered FC-PQ with pass limit 16 and a starvation clamp.
- An evaluation (§@sec:eval) of what these policies buy and cost. `ces-k64-home` and `fc-remote` restore burden Jain 0.98--1.00 at 0.95--1.01× CES throughput. `fcpq-h16-home-c16` reaches service Jain 0.997. Burden-fair delegation runs #(SusOnLo)--#(SusOnHi)× `tokio::sync::Mutex` under sustained load; its 4--5× bursty margin is mostly an artefact of tokio's LIFO slot. The obvious alternative, a per-lock server task (actor), reaches throughput or burden fairness but not both. Service fairness lowers lock utilisation by an amount that the per-op overhead $o$ fully accounts for.

#par(first-line-indent: (amount: 1em, all: true))[We do not claim the first delegation lock for cooperative scheduling. CES states that claim for itself~@ces. Our subject is fairness, and the executor/lock split that it requires.]

// ---------------------------------------------------------------------------
= Background and Motivation <sec:motiv>

== Cooperative executors
A tokio-style executor~@tokio runs $W$ worker threads. Each has a local run queue, and there is one shared injector queue. An idle worker steals half of a random peer's queue. tokio adds two policies that matter here:
- A _LIFO slot_: a task woken from a worker is polled right after the current poll, cannot be stolen, and is capped at 3 in a row.
- A _cooperative budget_ of 128 units per poll, consumed by `tokio::sync::Mutex` acquires.
Tokio's fairness statement for its mutex covers only the tasks waiting for it~@tokio6049.

Our executor (§@sec:impl) is built from `async-task`~@asynctask and `crossbeam-deque`~@crossbeam. It has per-worker FIFO queues, an injector and a single run-next slot. Every 31 polls a worker drains up to $l e n \/ W+1$ injector tasks (tokio's share rule). In our workload every worker hosts an always-runnable bystander, so steal-when-idle never fires. We therefore add a rate-limited _balancing steal_: every $b$ polls, a busy worker takes half of a random peer's queue. Runs use $b=31$ ("b31") or no balancing ("b0").

== Locks
We use the taxonomy of CES~@ces.
- _Dispatch_ (`dispatch`; tokio, Kotlin, Boost): unlock enqueues the next waiter and the owner continues.
- _Inline_: unlock resumes the waiter recursively.
- _CES_ (`ces`): unlock reschedules the owner remotely and resumes the head waiter inline.
- _FC_ (`fc`)~@fc: clients publish closures, and whoever wins a `try_lock` runs a pass of up to $H$ of them.
- _FC-PQ_ (`fcpq`): FC that serves the lowest cumulative charged usage first.

== Workload and metrics
The protected structure is a `BTreeMap<u64,u64>` with 65 536 keys. A critical section (CS) is one insert plus a TSC-timed spin: 1 000 cycles for light clients and 8× for heavy clients, half the clients each.
- _Sustained_: 64 clients with 4 000 cycles of parallel work between operations.
- _Bursty_: 16 clients with 32 000 cycles.
$W$ bystander tasks spin 1 000 cycles and then yield. Three metrics are Jain indices~@jain, $J=(sum x_i)^2 \/ (n sum x_i^2)$:
- _service Jain_, over per-client CS cycles;
- _burden Jain_, over per-worker combining cycles;
- the _combiner-worker bystander p99_, the p99 schedule-to-poll delay on the worker with the most combining cycles, against the maximum p99 of the other workers.
A bystander first polled after the window is _censored_ ("cens." in the tables): it never ran.

#fig("figures/fig-motivation.svg", wide: true)[Both subversions at $W=8$, heavy 8×. (a) Share of combining cycles per worker, workers sorted by share, sustained b0; the legend gives burden Jain. (b) p99 bystander schedule-to-poll delay on the combiner worker (filled) and the maximum over the other workers (open), same runs; CES's combiner-worker bystander is never polled in the 2~s window. (c) Per-client CS cycles over the mean, sustained b31; dashed: FIFO theory (equal operations per client at `dispatch`'s measured costs); the legend gives service Jain. `ces-k64`, `ces-k64-home`, `fc-remote` and `fcpq-h16-home-c16` are the policies of §@sec:design. All runs are from the 09-28 window except `fcpq-h16-home-c16` (hollow, 09-29). Markers and bars are medians of 3 repeats; error bars span [min, max].] <fig:motiv>

== Measured subversion
@fig:motiv shows both subversions at 8 workers. The full matrix covers $W in {4,8,16}$, heavy $in {1,8}$ and $b in {0,31}$ (432 runs); @tab:motiv in the appendix lists its $W=8$, heavy-8× cells.

#par(first-line-indent: 0pt)[*CES concentrates all combining on one worker.* In every saturated sustained cell CES has burden Jain exactly $1\/W$ (0.125 at 8 workers, 0.062 at 16), with or without balancing: one worker does all the combining (@fig:motiv a). The median chain is 819 200 inline hand-offs, i.e. one chain lasts the whole window. What happens to the tasks around it depends on balancing:]
- Without balancing, CES traps whatever sits in the combiner's queue: 5 clients (range 5--6 across repeats) and the bystander starve for 2 s (@fig:motiv b). Service Jain drops to 0.574#rng[0.557][0.627].
- With balancing, the combiner's bystander is stolen during warm-up and never returns. The burden shows up as eviction, not as delay.
Whether a CES chain ever ends is an executor property. An earlier executor version drained one injector task per 31 polls. At $W <= 8$ that let the wait queue empty periodically: burden Jain was 0.96 at 8 workers but 0.06 at 16.

#par(first-line-indent: 0pt)[*FC without a yield hands the lock to one client.* When the executor does not balance, the FC combiner's `run` completes synchronously. The waiters it served are woken into its own worker's queue, and the client loop never returns `Pending`. The same task therefore re-publishes and re-wins the election forever. Service Jain is $0.016=1\/64$ and 63 clients starve (bursty: 15 of 16). A yield after combining fixes the starvation. It does not fix placement: at b0, all clients converge on the combiner's worker (burden 1/W) and throughput is 0.235 instead of 0.390 Mops/s with balancing.]

#par(first-line-indent: 0pt)[*FIFO service is proportional to cost.* @fig:motiv c shows the per-client result. Under `dispatch` and `tokio::sync::Mutex` every light client receives 0.28× and every heavy client 1.71--1.72× the mean lock time, the FIFO prediction $2 C_L \/ (C_L+C_H)$ and $2 C_H \/ (C_L+C_H)$. @tab:fifo applies the two-class model $J(x)=(1+x)^2 \/ (2(1+x^2))$ to each lock's measured costs, where $x$ is the light:heavy per-client service ratio. Service Jain follows from the costs to three digits for `dispatch`, CES, FC and `tokio::sync::Mutex`. Equal service would need 6.0--6.5 light operations per heavy one.]

#tbl("tables/tab-fifo.typ", colsep: 3pt)[FIFO service fairness, $W=8$, sustained, b31 (09-28 window). $"CS"_(L,H)$ are critical-section cycles per op (TSC). The model uses $x="L:H" dot "CS"_L \/ "CS"_H$. The last column is $"CS"_H$/$"CS"_L$.] <tab:fifo>

#par(first-line-indent: 0pt)[*Off-the-shelf async locks.* In tokio, only `tokio::sync::Mutex` serves every client (§@sec:q3). `async-lock`~@asynclock collapses to a single client in 11 of 12 matrix runs (63 of 64 or 15 of 16 clients starved). With the LIFO slot on it collapses in 20 of 24 runs, and with the slot off in 0 of 12. `std` and `parking_lot` mutexes block the worker thread. The first $W$ clients to be polled keep the $W$ workers for the whole run, so starved clients equal clients $- W$, and all $W$ bystanders starve.]

// ---------------------------------------------------------------------------
= Design Goals <sec:goals>

Following SCL~@scl, we state the properties a delegation lock in a cooperative executor should have.
#[
#set enum(numbering: n => strong[G#n])
+ *Burden fairness.* Combining cycles are spread across workers (burden Jain $>= 0.9$). The p99 of a bystander on the combiner worker is within one histogram bucket of the p99 elsewhere.
+ *Service fairness.* Clients receive equal lock _time_, not equal operations. This requires usage accounting (service Jain $>= 0.95$ under 1:8 cost heterogeneity).
+ *Work conservation.* Throughput is no worse than FIFO delegation. The fairness policy adds no idle lock time.
+ *Executor independence.* The lock asks the executor only for wake placement. It does not need priorities, preemption or scheduler callbacks.
+ *Bounded wait.* No request waits more than a fixed number of passes, whatever its usage.
]
G2 and G3 conflict in one measurable way: fairness shifts the served mix toward cheap operations, and each operation carries a fixed lock cost. We quantify this conflict in §@sec:q5 rather than hide it in an ops/s number.

// ---------------------------------------------------------------------------
= Design <sec:design>

#figure(placement: top, scope: "parent", kind: image, contract,
  caption: [The executor contract and the two burden policies. (a) A lock may ask for each wake to be placed _Inline_ (the waking worker's run-next slot), _Remote_ (the shared injector) or _Home_ (the inbox of the worker that last polled the wakee). (b) CES resumes waiters inline on one worker; `ces-k64-home` ends the chain after $K=64$ hand-offs and hands ownership to the next waiter on its home worker, so the ex-combiner drains its own queue. (c) An FC combiner serves at most $H$ closures, wakes the served waiters remotely (`fc-remote`) and yields once after combining, so the tasks queued on its worker run before it competes again.]) <fig:contract>

== Executor contract
A `Waker` can only enqueue its task, so it carries no placement. The executor exposes a thread-local placement hint, which the schedule callback reads and resets (@fig:contract a):
- `wake_with(Inline, w)` puts the task in this worker's run-next slot. The slot is polled as soon as the current poll returns, before the local queue and before stealing. It has no anti-starvation cap; that is deliberate, so that unbounded CES chains remain observable.
- `wake_with(Remote, w)` pushes the task to the injector and unparks one worker.
- `wake_with(Home, w)` pushes the task to the inbox of the worker that last polled it.
- `reschedule_self_remote(cx)` marks the _current_ task `Remote`. `async-task` defers the schedule callback until the poll returns `Pending`.
Tokio's LIFO slot is an implicit, capped `Inline` for every wake issued on a worker. The contract makes that choice explicit and per-wake. It adds no policy to the executor beyond what the lock requests (G4).

== Burden: CES chain bound and break placement
A _chain_ is the sequence of inline resumes on one worker since that worker last polled anything else (@fig:contract b). `ces-k`$K$ ends a chain after $K$ hand-offs; `ces-t`$T$ ends it after $T$ cycles. At that point the unlocker does not resume the head waiter inline. It hands ownership to the head waiter with a _break placement_ and continues, so the worker drains its own queue when the poll returns. With the default break placement, the woken owner lands in the ex-combiner's own queue and the next chain restarts there. The `-home` suffix sends it to its home worker's inbox, which moves the combiner role (§@sec:q1).

== Burden: FC yield, placement and pass limit
Every FC client owns one node, allocated once. A request writes a closure pointer and a trampoline into the node and pushes the node onto a Treiber stack. The client then tries the combiner flag. The winner drains the stack into the policy queue and runs at most $H$ closures (default 64). Each served waiter is marked `COMPLETE` (release/acquire) and woken with the lock's _wake placement_.

Losers return `Pending`; a waiter never spins. At the end of a pass the combiner handles three cases:
- Queue empty: release the flag and re-check the stack (SeqCst on both sides).
- Own request served: release the flag and wake the policy's next candidate, which re-runs the election.
- Own request still pending: keep combining.
_Yield-after-combine_ (default on) makes a poll that won the election return `Pending` once, having woken itself behind the waiters it just served (@fig:contract c). Preemption gives OS-thread FC the same effect. `-remote` and `-home` set the placement of every wake the lock issues.

== Service: usage-ordered FC-PQ
FC-PQ keeps pending requests in a binary min-heap keyed by (cumulative charged cycles, arrival). The combiner charges each closure the `rdtscp` cycles around its execution, and the charge persists in the client's node. Two rules modify the key:
- A _newcomer_, meaning a client never served before, enters at the lock's running mean cost per request. Zero, the minimum and the median are alternatives.
- The _starvation clamp_ runs at every pass start. An entry that has waited more than $c$ passes (default 8) has its key lowered to the current heap minimum until it is served. The accounting is untouched. This fixes a no-op in the reference implementation, which clamped after the pop.
The pass limit $H$ decides whether ordering matters at all. When $H >=$ the number of waiters, every pass admits everyone, ordering only permutes requests within a pass, and the share stays FIFO's.

#par(first-line-indent: 0pt)[*Why clamp 8 binds and clamp $>=$12 does not.* Take $H=16$ with $N_h=32$ heavy clients. A heavy client's service interval is $2(1+"L:H")$ passes. Under clamp 8 every heavy operation is a clamp promotion: #PromotedCEight of requests are promoted, which equals the heavy share of operations $1\/(1+3.64)$. The interval is therefore $(c+1)+0.28=9.28$ passes, where 0.28 is the return delay (parallel work plus wake). That gives L:H $=3.64$.]

Without the clamp, usage ordering settles where charged usage is equal. That happens at an interval of $2 times 6.51=13.0$ passes. A clamp therefore binds only if $c+1.28<13.0$, i.e. $c <= 11$. At clamp 16 it promotes at most 0.035 % of requests, all of them in the wait tail.

Service Jain follows from the two-class model with $x="L:H" dot "CS"_L \/ "CS"_H$. The model predicts 0.9395 at clamp 8 (measured 0.940) and 0.9970 at clamp 16 (measured 0.997). $J >= 0.95$ needs $x >= #XNinetyFive$, i.e. L:H $>= #LHNinetyFive$. By the binding rule that means $c >= 9$. For clamps 9--11 the model predicts $J approx 0.963$, 0.981 and 0.992. We did not run them.

Jain stops at 0.997 rather than 1 because the charge window includes the trampoline that moves the closure and its result. That is about 193 cycles per operation that the harness's CS timer does not see.

#par(first-line-indent: 0pt)[*Utilisation.* Let $macron(C)$ be the mean CS cycles per operation of the served mix and $o=(T-C S) \/ o p s$ the per-operation cycles not spent in a CS. For FC-family locks under sustained load, where some combiner is busy 98.8--98.9 % of the window, $o$ is the lock's per-op cost: in-pass administration plus hand-off gap. Then lock utilisation is $C S \/ T = macron(C) \/ (macron(C)+o)$. This is an identity. Its content is empirical: $o$ is nearly independent of the clamp setting (#OCEight cycles at clamp 8, #OCSixteen at clamp 16). Moving the mix toward cheap operations lowers $macron(C)$ (#CbarCEight to #CbarCSixteen cycles) and therefore lowers utilisation.]

== The obvious alternative: an actor
`actor` spawns one server task per lock, lazily with the first request. Each server poll drains the request stack, serves up to 64 closures in FIFO order and yields. The server parks when there is nothing to serve. `actor-inline` changes three placements: the server is woken into the publisher's run-next slot, it yields `home`, and it wakes clients `remote`. The actor idiom is what an application would write without a delegation lock.

// ---------------------------------------------------------------------------
= Implementation <sec:impl>

The locks, executor and harness are written in Rust. @tab:loc gives the size of each part.

#par(first-line-indent: 0pt)[*Executor.* Tasks are `async-task` tasks. Each worker has a `crossbeam-deque` FIFO and a run-next slot; there is one shared `Injector`. Idle workers park through a SeqCst-fenced registration protocol that forbids lost wake-ups. Workers are pinned to distinct physical cores.]

#par(first-line-indent: 0pt)[*Requests.* Clients never allocate per request. Each owns one node, and the lock keeps all nodes alive so that a combiner never touches a freed node. `Run` futures hold the closure and the result slot inline. Completion is a release store of `COMPLETE`, and the owner reads it with an acquire load. All locks share one `rdtscp` counter with the harness.]

#par(first-line-indent: 0pt)[*Checks.* By the lock contract, no lock allocates per request in steady state: per-client nodes and in-place heaps or queues only. A counting-allocator probe in steady state measured 0~allocations per op for `fc` and `actor`. The executor queues allocate 0.012--0.016 per op for `fc-remote` and 0.016--0.025 for `actor-inline` (one `Injector` block per 63 remote wakes). `dispatch`, `ces` and `fcpq` were not probed #todo[allocation probe for `dispatch`/`ces`/`fcpq` not recorded].]

The `actor` unit tests exercise mutual exclusion with overlap detection, the absence of lost wake-ups across idle transitions, and FIFO order. They pass in release builds and under ThreadSanitizer (0 reports). We have no TSan record for the other locks #todo[TSan runs for `dispatch`/`ces`/`fc`/`fcpq` not recorded]. `coro-bench --sanity` and `tokio-bench --sanity` (map length equals total ops) pass for all locks.

#tbl("tables/tab-loc.typ")[Lines of Rust: non-blank, non-comment lines, with lines from the first `#[cfg(test)]` on counted as tests. Counted by `make_tables.py`.] <tab:loc>

// ---------------------------------------------------------------------------
= Evaluation <sec:eval>

#par(first-line-indent: 0pt)[*Setup.* The machine has 2× Intel Xeon Gold 6438M (32 cores per socket, SMT on, 128 logical CPUs), Linux 6.17.7, a 2.20 GHz TSC and `rustc` 1.100.0-nightly with `--release`. $W$ workers are pinned to logical CPUs $0..W-1$, one per physical core. Each run has a 200 ms warm-up and a 2 s window. Each configuration is repeated 3 times, with repeats as the outermost loop, and runs execute one at a time under a measurement lock. We report medians with [min, max] over the repeats, and we do not interpret differences inside the spread. Latencies are histogram bucket lower bounds at 6 % resolution. One unrelated single-threaded process ran during the 09-28 matrices. #todo[commit id of the measured binaries not recorded; binaries are identified by sha256 in FINDINGS.md]]

#par(first-line-indent: 0pt)[*Two measurement windows.* From 2026-09-29 00:07:54 UTC, CPUs 0--15 were capped at 3.0 GHz. On 09-28 they ran at about 3.69 GHz turbo. Spins are TSC-timed, so the cap slows only non-spin work: a light CS costs 1 365--1 376 instead of 1 303 cycles. Every table row is labelled with its window, and ratios are formed only within one window unless marked otherwise. Every figure panel names its window; where one axis shows both, 09-29 data are drawn hollow.]

The same binary paths re-run on 09-29 were 1.4--3.4 % slower than their 09-28 cells (`dispatch`, `ces-k64-home`, `fc-remote`). This is outside both spreads, and service Jain rose by 0.004--0.007. We attribute the drift to the cap [inference; the actor entry that measured it predates the diagnosis]. Cross-window ratios are therefore uncertain by about 3 %.

== Q1: Does placement restore burden fairness? <sec:q1>

#fig("figures/fig-burden.svg", wide: true)[Burden fairness, phase 3, heavy 8× (09-28 window). Top: burden Jain over workers; bars are $W=8$, diamonds $W=16$ (measured for `ces` and `ces-k64-home` only); dashed: goal G1. Bottom: bystander p99 schedule-to-poll delay (µs, log scale) on the combiner worker (filled) and the maximum over the other workers (open). ▲: the combiner worker's bystander was never polled in the 2~s window (censored); ×: no bystander was left on the combiner worker (evicted by balancing). Medians of 3 repeats; error bars span [min, max]. Numbers and throughput: @tab:burden.] <fig:burden>

@fig:burden gives the answer; @tab:burden in the appendix lists the numbers and each variant's throughput relative to CES.

_A chain bound alone does nothing for burden at b0._ `ces-k64` keeps burden at 0.125 (left column of @fig:burden). The ex-combiner drains its own queue, the next acquirer is again one of its own clients, and the combiner's bystander p99 is 171 µs, the length of one 64-hand-off chain. The bound ends starvation but not concentration.

_The bound plus a home chain break fixes it._ `ces-k64-home` has burden Jain 0.993--0.999 in every cell (8 and 16 workers, sustained and bursty, b0 and b31). Its combiner-worker bystander p99 equals the others' (2.6--2.9 µs sustained; 44.7 and 14.9 µs bursty at 8 and 16 workers), and no task starves: in @fig:burden its filled and open markers coincide in all four columns. It costs 0.98--0.99× CES throughput sustained and 0.95--0.99× bursty.

_Bursty load needs no bound._ In bursty cells CES chains already end naturally (p50 12 hand-offs). The bound only trims the tail from a maximum of 152--191 to 64. The cycle budget (`ces-t64000-home`) breaks every $tilde.op$12 hand-offs even where the queue would have continued, and costs 5--8 %.

_For FC, `remote` beats `home`._ `fc-remote` removes the b0 one-worker convergence of `fc` (burden 0.981#rng[0.967][0.996] sustained, 1.000 bursty) at the balanced throughput level. `fc-home` reaches only 0.744 at b0 sustained. The election is sticky: the ex-combiner's own clients are woken locally and win the next `try_lock`. In bursty mode `home` is nevertheless faster (1.09--1.13× CES), because served waiters resume their parallel work on their own worker.

_Bystander delay did not exceed the other workers' under balancing._ Our pre-registered hypothesis was that the combiner-worker bystander p99 would exceed the other workers' p99 by more than one pass ($H times$ mean CS). It is refuted under balancing in all 12 bursty cells for all five combining variants: the two p99s are equal within one bucket (right column of @fig:burden), for example 44.7 against 44.7 µs for CES at 8 workers, with a 136.6 µs pass. Without balancing the effect is starvation, not delay.

== Q2: Does usage ordering restore service fairness? <sec:q2>

#tbl("tables/tab-service.typ", wide: true)[Service fairness, $W=8$, sustained, b31. Upper block: phase 3 (09-28). Lower block: same-window A/B and clamp sweep (09-29, 3.0 GHz cap); `-c`$N$ is `fcpq-h16-home` with starvation clamp $N$ (0 = off) and newcomer init _mean_. "model $J$" is the two-class model applied to the measured L:H and per-class costs. L:H, util and non-CS/op are medians (max spread 5 % of the median). util $=$ CS cycles / window. non-CS/op $=$ (window $-$ CS cycles) / ops; only for the FC-family rows, whose combiner is busy almost the whole window, is it the per-op lock cost $o$ of §@sec:design; for `dispatch` and `ces` it also contains lock-idle and hand-off time. Max wait is in combining passes. Do not compare rows across the two blocks.] <tab:service>

#fig("figures/fig-service.svg", wide: true)[Service fairness of FC-PQ, $W=8$, sustained, b31. (a) Pass limit $H$ (09-28 window): bars are measured service Jain, ticks the two-class model; dashed: goal G2. (b--f) Starvation-clamp sweep of `fcpq-h16-home`, newcomer init _mean_ (09-29 window): (b) service Jain with the model (ticks) and the confirmation cells (hollow; $W=8$ b0 and $W=16$ b31 were run at clamps 8 and 16 only); (c) light:heavy ops against the ratios the model needs for Jain 0.95 and 1; (d) throughput; (e) heavy-client run latency; (f) worst queue wait, with $c+1$ passes marked. Dashed green: same-window `fc-remote`. Medians of 3 repeats; error bars span [min, max].] <fig:service>

@fig:service a shows that the pass limit decides whether usage ordering has any effect. `fcpq` with $H=64$ admits every waiter in every pass, so it reaches only 0.706. $H=8$ gives 0.763, $H=16$ 0.863, and $H=16$ with `home` wakes 0.932. The remaining FC-PQ knobs leave service Jain at or below `fcpq`'s level: pass budget `t16000` 0.661, rotation 0.708, combining credit 0.693, max-usage election 0.707 (@tab:service lists the main rows). None of them changes the admission set.

At $H=16$ with `home` wakes, the 8-pass clamp is the bound: L:H stays at 3.64, as §@sec:design predicts, below the #LHNinetyFive that Jain 0.95 needs (@fig:service c). Lengthening the clamp to 16 lifts service Jain to 0.997 at $W=8$ b31, $W=8$ b0 and $W=16$ b31 (@fig:service b; @tab:confirm in the appendix). L:H rises to 5.45--5.53, and ops throughput rises by 13 % (@fig:service d; 0.540→0.612 Mops/s, same window). Longer clamps change nothing further: clamp 32 and no clamp give the same Jain, L:H and throughput.

The price is heavy-client latency (@fig:service e). Heavy p50/p99 grow from 253/357 µs to 328/626 µs, and light p99 from 164 to 343 µs. The worst queue wait grows with the clamp (@fig:service f): 17 passes at clamp 16 and 33 at clamp 32. With the clamp off it is 106#rng[54][193] passes, with heavy maximum latencies of 1.5--4.3 ms. We therefore take `fcpq-h16-home-c16`: it has the fairness of clamp-off and a bounded worst case (G5).

Two settings had no effect. The newcomer rule fires on only 64 of about 1.2 M requests per run, all during warm-up when every usage is near 0, so mean, zero, min and median initialisation are indistinguishable. In bursty load every knob is inert (Jain 0.697 at $W=8$): with at most 16 waiters and 7.8 ops per pass, every pass serves everyone pending. Usage ordering can only act when the backlog exceeds $H$.

_Side effect on burden._ Clamp 16 also fixes a burden problem at b0, from 0.501#rng[0.500][0.544] to 0.970#rng[0.969][0.974]. Our explanation [inference]: under clamp 8 the pass-end hand-off almost always goes to a light client, and at b0 light clients live on 4 of the 8 workers.

== Q3: Against real tokio locks <sec:q3>

#fig("figures/fig-xrt.svg", wide: true)[Throughput against tokio 1.53.1, relative to `tokio-mutex` (LIFO slot on) in the same cell; log scale (09-28 window; coro locks b31). Hatched: `tokio-mutex` with the LIFO slot disabled, as off/on within a throwaway `tokio_unstable` build whose LIFO-on runs are 0.96--1.00× the release binary. Black ticks on the delegation bars: the same throughput divided by LIFO-off `tokio-mutex`. Numbers above bars: clients that completed no operation in 2~s. `fcpq-h16-home` is clamp 8 and counts cheap light ops, so it is not a like-for-like throughput reference. Medians of 3 repeats; error bars span the numerator's [min, max].] <fig:xrt>

#tbl("tables/tab-xrt-thr.typ", wide: true, colsep: 3.5pt)[Throughput against tokio 1.53.1 (09-28 window; coro rows b31), the numbers of @fig:xrt. ×tm is the ratio to `tokio-mutex` in the same cell. *The bursty ratios of 4--5× are largely due to tokio's LIFO slot.*] <tab:xrt>

#tbl("tables/tab-xrt-fair.typ", wide: true, colsep: 3.5pt)[Service Jain and starved clients/bystanders for the runs of @tab:xrt. `std-mutex` and `parking-lot` are really $W$ pinned threads on a blocking mutex (see text).] <tab:xrtfair>

We ported the workload unchanged to tokio's multi-thread runtime (`tokio-bench`): same key stream, costs, histogram and pinning. @fig:xrt summarises the comparison; @tab:xrt and @tab:xrtfair give the numbers.

#par(first-line-indent: 0pt)[*The executor is not a weak baseline.* Coro `dispatch` runs at 0.90--0.98× `tokio-mutex` (grey bars in @fig:xrt). The coop budget never forced a yield, and removing it (`-unconstrained`) moves throughput by 0--1 %.]

#par(first-line-indent: 0pt)[*Sustained load.* The burden-fair delegation locks run #(SusOnLo)--#(SusOnHi)× `tokio-mutex` (blue and green bars). They starve no client and no bystander and keep FIFO-level service Jain (0.653--0.678, against 0.663--0.672 for `tokio-mutex`).]

#par(first-line-indent: 0pt)[*The LIFO-slot caveat.* Bursty ratios of #(BurOnLo)--#(BurOnHi)× come mostly from tokio's LIFO slot, not from the lock. A mutex hand-off wake lands in the unlocker's LIFO slot, and the new owner waits there behind the unlocker's parallel work. Disabling the slot lifts `tokio-mutex` 3.4--3.7× in bursty mode and 1.08× in sustained mode (hatched bars; @tab:lifo). Against that configuration (black ticks), `ces-k64-home` and `fc-remote` run #(BurOffLo)--#(BurOffHi)× bursty and #(SusOffLo)--#(SusOffHi)× sustained. The advantage that holds under either tokio configuration is 1.5--1.6× sustained and about 1.2--1.4× bursty.]

#par(first-line-indent: 0pt)[*Blocking mutexes.* `std-mutex` and `parking-lot` beat every async lock at $W=8$ bursty (5.02× and 4.90×). In that cell they serve only 8 of the 16 clients and starve all bystanders (@fig:xrt, @tab:xrtfair), so the ratio is not like-for-like. Elsewhere the delegation locks beat `std-mutex` 1.80--3.28×.]

#par(first-line-indent: 0pt)[*Missing comparison.* The two runtimes were only measured together in the 09-28 window. `fcpq-h16-home-c16` (09-29) has no same-window tokio reference #todo[same-window tokio baseline for `fcpq-h16-home-c16`].]

== Q4: Is a server task enough? <sec:q4>

#fig("figures/fig-actor.svg")[Actor control: throughput relative to `fc-remote` in the same cell, against burden Jain. Shaded: the goal (burden Jain $>= 0.9$ at $>= 0.95×$ `fc-remote`). Marker shape gives the cell. At $W=8$ b31 all runs are from the 09-29 window. In the b0 and $W=16$ cells the references are stored 09-28 cells, so the actor ratios there (hollow) cross windows and understate the actor by up to about 3 %. Error bars span burden Jain's [min, max]. Numbers: @tab:actor.] <fig:actor>

@fig:actor compares the two actor variants with the delegation locks; @tab:actor in the appendix gives the numbers. Plain `actor` is burden-fair only because the executor's balancing steal keeps moving the server: burden Jain is 0.990 sustained and 0.999 bursty at $W=8$ b31. It runs 0.91× same-window `fc-remote` sustained and 0.54× bursty. Served clients are woken into the server's queue ahead of the yielded server, so each pass waits for their parallel work [inference]. At b0, `actor` is a one-worker system: burden 0.125, 0.59× sustained, 0.19× bursty, and a server-worker bystander p99 of 268--283 µs.

`actor-inline` restores throughput (1.04× sustained, 0.96× bursty at $W=8$ b31), but under sustained load the server never parks. A server that never parks is never re-placed by a wake, so only balancing steals move it. Burden is 0.179#rng[0.125][0.267] at $W=8$ and 0.612#rng[0.556][0.651] at $W=16$. At b0, its worker's bystander p99 is 156 µs, about one 64-request pass.

No actor variant reaches the goal corner of @fig:actor in the sustained cells: each is either fast or burden-fair. `ces-k64-home` and `fc-remote` reach it, because they move the combiner role by construction. A server task would need the same explicit migration.

== Q5: Costs and limits <sec:q5>

#fig("figures/fig-util.svg")[Lock utilisation (CS cycles / window) against the per-op cycles not spent in a critical section, $o$, for FC-family locks at $W=8$, sustained, b31. Curves: the identity util $= macron(C) \/ (macron(C)+o)$ for the FIFO mix ($macron(C)$ of same-window `fc-remote`), the mix at the Jain-0.95 bound, and the clamp-16 mix. Purple points are FC-PQ settings. Filled: 09-28; hollow: 09-29. Star: the $o$ that utilisation 0.80 needs at Jain 0.95. Error bars span [min, max].] <fig:util>

#par(first-line-indent: 0pt)[*Utilisation.* Service fairness lowers the fraction of time the lock spends doing work (@fig:util). On 09-28, utilisation drops from #UtilFcOld (`fc`) to 0.725 (`fcpq-h16-home`). On 09-29 it drops from #UtilFcRemoteAB (`fc-remote`) to #UtilCEight (clamp 8) and #UtilCSixteen (clamp 16), while a combiner is busy 98.8--98.9 % of the window. Each point sits on the curve of its own mix: FC-PQ moves left-to-right only a little ($o$) but drops to curves of cheaper mixes ($macron(C)$). The lock's cost is $o=#OCSixteen$ cycles/op, 96--97 % of it in-pass administration. Same-window `fc-remote` has $o=#OFcRemoteAB$, and the lowest $o$ of any fc-family sustained cell is `fc` on 09-28 at turbo clock, with $o=#OFcOld$.]

At the fairness bound $J=0.95$ (L:H $= #LHNinetyFive$), the mix costs $macron(C)=#CbarNinetyFive$ cycles, so utilisation 0.80 requires $o <= #ONeededEighty$ cycles/op (the star in @fig:util). That is 39 % below `fcpq-h16-home` and 25 % below `fc-remote`. Even $o=#OFcOld$ would give only #UtilAtOFcOld. The ops/s gain of FC-PQ is therefore partly an accounting effect: the lock does less work per second while completing more operations. Our first candidate for the cost [inference] is home wakes: on 09-28, `fcpq-h16` with default placement had 769 admin cycles/op against 1 005 for `-home`. We have not broken $o$ into drain, heap, rekey and wake costs #todo[breakdown of $o$ into drain / heap / rekey / home-wake cycles].

#par(first-line-indent: 0pt)[*Usage ordering alone costs little.* At heavy ratio 1, FC-PQ's throughput is within 0--3 % of FC.]

#par(first-line-indent: 0pt)[*Limitations.*]
- Bystanders are always runnable, so steal-when-idle never fires. The balancing steal is our substitute, and it bounds the measurable bystander delay. Real bystanders are I/O-driven. With idle gaps, stealing could rescue trapped tasks, and it would also move a never-parking actor server. We have not tested this.
- The workload is closed-loop with one await per iteration and no idle clients. Late joiners and clients returning from idle were not exercised, and no newcomer rule handles idle return.
- Three repeats give min/max spreads, not confidence intervals.
- The clock cap splits the data into two windows (above).
- The workload is one data structure plus spins; there is no application benchmark.

// ---------------------------------------------------------------------------
= Related Work <sec:related>

#par(first-line-indent: 0pt)[*Scheduler-cooperative locks.* SCL and its user-space variant U-SCL~@scl name _scheduler subversion_ for OS threads: lock usage rather than scheduler share decides CPU allocation. They fix it with usage accounting and lock-slice penalties. CFL~@cfl schedules lock occupation by cgroup share and priority, and is NUMA-aware. Our service-fairness goal is the same idea (fair lock _time_). We apply it to a combiner in a cooperative executor, where the combiner also decides whose thread pays.]

#par(first-line-indent: 0pt)[*Delegation locks.* FC~@fc is starvation-free and linearizable. It lists the number of consecutive combining rounds as a knob and does not measure the combiner's own delay. TCLocks~@tclocks bound combining batches at 1 024 waiters for "long-term fairness". They give no fairness metric and defer charging the waiter for combining. Neither measures a per-thread combiner share. The FC dispatcher poster~@fcdispatch rotates the dispatcher role, which mitigates burden, but does not measure the cost.]

#par(first-line-indent: 0pt)[*CES.* CES~@ces states that it is the first delegation-style lock for cooperatively scheduled tasks. It evaluates throughput only, at uniform CS cost, and notes qualitatively that the CS stays on one thread per resource. We build on its inline/remote exchange and add what a fairness analysis needs: chain bounds with break placement, the burden metric, and usage-ordered service.]

#par(first-line-indent: 0pt)[*Async runtimes and locks.* The tokio mutex~@tokio is a FIFO semaphore whose hand-off wake goes through the LIFO slot. Reading its source, this is a bounded inline-after-poll hand-off on the unlocker's worker [inference, consistent with the LIFO side check of §@sec:q3]. Its fairness scope is only the waiters~@tokio6049. `async-lock`~@asynclock relies on a 0.5 ms starvation hand-off, which never fires when the notified waiter is never polled.]

#par(first-line-indent: 0pt)[*Preemptive request scheduling.* Shinjuku~@shinjuku preempts every 5--15 µs to handle heterogeneous request costs, and places contended locks out of scope. Perséphone's DARC~@persephone reserves cores per request type and assumes independent requests. They handle cost heterogeneity in the request scheduler. We handle it inside the lock, where a cooperative executor has no preemption to fall back on.]

// ---------------------------------------------------------------------------
= Conclusion and Future Work <sec:concl>

A delegation lock in a cooperative executor decides two things the executor cannot see: which worker pays for critical sections, and which client is served next. Left at their defaults, the first concentrates all combining on one worker (burden Jain $1\/W$) or lets one client take over, and the second gives FIFO service proportional to cost (Jain 0.65--0.66). Both can be fixed inside the lock with only wake-placement hints. A chain bound with home break (`ces-k64-home`) or remote wakes with a pass-end hand-off (`fc-remote`) restore burden Jain 0.98--1.00 at CES throughput. Usage-ordered FC-PQ with $H=16$, home wakes and a 16-pass clamp reaches service Jain 0.997 with a 17-pass wait bound. Burden-fair delegation runs 1.5--1.6× `tokio::sync::Mutex` under sustained load in either tokio configuration. Service fairness costs utilisation (0.84→0.68), because the per-op lock cost $o$ is fixed.

Future work:
- I/O-driven, intermittently runnable bystanders, where steal-when-idle exists.
- A remote-yield actor, i.e. explicit server migration.
- Clamps 9--12, to trace the predicted Jain/latency frontier.
- Reducing $o$ below 703 cycles/op, starting with home wakes.
- Real applications with I/O between critical sections.

#[
#set text(size: 9pt)
#bibliography("refs.bib", style: "ieee", title: "References")
]

// ---------------------------------------------------------------------------
#counter(heading).update(0)
#set heading(numbering: "A.1")
= Supplementary Tables <sec:appendix>

The tables below give the numbers behind @fig:motiv, @fig:burden, @fig:service b, @fig:xrt and @fig:actor. They are generated by `make_tables.py` from the same result files.

#tbl("tables/tab-motivation.typ", wide: true)[The problem, phase-2 matrix, $W=8$, heavy 8× (09-28 window). Service $J$ is over clients and burden $J$ over workers. The p99 columns are bystander schedule-to-poll delays in µs on the combiner worker and the maximum over the other workers. The last column counts starved clients and bystanders (0 ops in 2~s). `fc-noyield` is FC without the cooperative yield of §@sec:design.] <tab:motiv>

#tbl("tables/tab-lifo.typ", wide: true, colsep: 2.5pt, placement: bottom)[tokio LIFO slot on/off (Mops/s; throwaway `tokio_unstable` build whose LIFO-on runs are 0.96--1.00× the release binary; 09-28 window): the hatched bars and black ticks of @fig:xrt. The right-hand columns divide the coro throughputs of @tab:xrt by LIFO-off `tokio-mutex`.] <tab:lifo>

#tbl("tables/tab-clamp-confirm.typ", colsep: 3pt, placement: none)[Clamp 8 vs. 16 for `fcpq-h16-home` in the confirmation cells (09-29 window). Heavy p99 is run latency in µs; L:H, p99 and burden $J$ are medians.] <tab:confirm>

#tbl("tables/tab-burden.typ", wide: true)[Burden fairness, phase 3, heavy 8× (09-28 window); the data of @fig:burden. The p99 columns are bystander delays in µs. "/`ces`" is the throughput ratio to plain CES in the same cell. `ces` rows with "--" combiner p99 have no bystander left on the combiner worker (evicted by balancing).] <tab:burden>

#tbl("tables/tab-actor.typ", wide: true)[Actor control; the data of @fig:actor. Upper blocks: same-window (09-29) references. Lower blocks: b0 and $W=16$, where the `fc-remote` reference is a stored 09-28 cell. Ratios marked † cross windows (actor 09-29 vs. reference 09-28) and understate the actor rows by up to about 3 %.] <tab:actor>
