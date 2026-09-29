#import "lib.typ": rng, fit
#import "tables/numbers.typ": *
#import "figures/contract.typ": contract

// A number or result the text wants but no FINDINGS.md entry or JSON provides.
#let todo(body) = text(fill: red, weight: "bold")[\[TODO: #body\]]

#let title = [Delegation Locks Are Hidden Schedulers:\ Combiner Burden and Service Unfairness in Cooperative Executors]

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
    Delegation (combining) locks run other tasks' critical sections on one thread. We show that, in a cooperative, work-stealing coroutine executor, such a lock is a _hidden scheduler_: it decides which worker spends its time on critical sections and which client is served next, and the executor sees neither decision. The first decision causes _combiner burden_: a Combine-and-Exchange (CES) lock keeps one worker combining for the whole 2 s window (burden Jain index $1\/W$). The second causes _service unfairness_: with a 1:8 cost mix, every FIFO lock we measured, `tokio::sync::Mutex` included, gives service in proportion to cost (service Jain 0.65--0.66). Both decisions can be made fair inside the lock, given only wake-placement hints. A chain bound with a home-worker break (CES) and remote wakes (FC) restore burden Jain 0.98--1.00 at 0.95--1.01× CES throughput; usage-ordered FC with a 16-pass starvation clamp reaches service Jain 0.997 with a worst queue wait of 17 passes. Under sustained load the burden-fair locks run #(SusOnLo)--#(SusOnHi)× `tokio::sync::Mutex` (tokio 1.53.1), #(SusOffLo)--#(SusOffHi)× with its LIFO slot disabled; a per-lock server task cannot match them. Fairness costs utilisation: a per-operation overhead of about 1 150 cycles lowers it from 0.84 to 0.68.
  ]
]

// ---------------------------------------------------------------------------
= Introduction <sec:intro>

Coroutines are back in wide use: many modern languages support cooperative multitasking~@ces. Runtimes such as tokio~@tokio multiplex many tasks onto a few worker threads. A task runs until it returns `Pending`, and workers balance load by stealing. Fairness guarantees are weak and cover only runnable tasks, or only a lock's waiters~@tokio6049. A lock in such a runtime is an _async mutex_: a waiter suspends, and the unlocker wakes it through the scheduler. CES~@ces observed that this puts a scheduler round trip on the critical path. It instead resumes the next waiter inline on the unlocking thread, so contended critical sections stay on one thread. CES is thus a delegation lock, like flat combining (FC)~@fc and TCLocks~@tclocks. It reports throughput only.

Patel et al.~@scl showed that ordinary locks _subvert_ the OS scheduler: lock hold time, not the scheduler's share, decides who gets CPU time. A delegation lock in a cooperative executor goes further: it is a _hidden scheduler_. It decides _where the combiner runs_ and _whom it serves next_, and the executor sees neither decision. Left at its default, each decision causes one problem. _Combiner burden_ is the cost that combining puts on the combiner's worker and on every task queued behind it. _Service unfairness_ is lock time that follows critical-section cost instead of being shared equally.

As a simple example, take $W=8$ workers, 64 clients, half with 8× the critical-section (CS) cost of the other half, and one always-runnable bystander per worker (@fig:motiv). With CES the unlocking worker stays combiner while the wait queue is non-empty, which under sustained contention is the entire 2 s run: burden Jain over workers is $1\/W$, and the bystander on that worker is never polled (@fig:motiv a, b). Without executor balancing, FC without a cooperative yield is worse: the combining task never returns `Pending`, so 63 of 64 clients do no work for 2 s. Service is unfair even under strict FIFO, which gives equal operations and hence lock time proportional to cost (@fig:motiv c). For the 1:8 mix the service Jain index is 0.653 (from the measured 1 306:8 324 cycles per op); `dispatch`, CES, FC and `tokio::sync::Mutex` all reproduce it to within 0.01 (@tab:fifo).

#par(first-line-indent: 0pt)[*The crux.* Can a delegation lock spread combining across workers and give clients equal lock time without losing throughput, and what is the least it must ask of the executor?]

In this paper, we argue that both decisions are lock policies, and that a lock needs from the executor only a placement argument on wakes (inline, remote, home). We expose it through a minimal contract and build three policies on it: a CES chain bound with home-worker chain break and FC yield-after-combine with remote wakes, for burden, and usage-ordered FC-PQ with pass limit 16 and a starvation clamp, for service. CES claims the first delegation lock for cooperative scheduling~@ces; our subject is fairness.

We find that bounding the combiner is not enough: the lock must also place the next owner, or the waiters it served, off the combiner's worker. Usage ordering helps only when the pass limit is below the backlog and the clamp does not bind. Burden-fair delegation runs #(SusOnLo)--#(SusOnHi)× `tokio::sync::Mutex` under sustained load, but its 4--5× bursty margin is mostly tokio's LIFO slot. A per-lock server task (actor) gets throughput or burden fairness, not both.

Our contributions are:
+ Measurements of both problems for `dispatch`, CES, FC, a usage-ordered FC (FC-PQ) and the tokio locks; FIFO service fairness follows from per-class costs to three digits (§@sec:motiv).
+ A minimal executor contract and lock policies on it, with a rule for when a starvation clamp binds (predicted Jain 0.9395, measured 0.940; §@sec:design).
+ An evaluation of the policies, including a utilisation identity that separates per-op lock cost from the served mix (§@sec:eval).

#par(first-line-indent: (amount: 1em, all: true))[The rest of this paper is organized as follows. §@sec:motiv demonstrates both problems and §@sec:goals turns them into goals. §@sec:design and §@sec:impl present the contract and policies, and §@sec:eval evaluates them. §@sec:discuss–§@sec:concl discuss limitations, related work and conclusions.]

// ---------------------------------------------------------------------------
= Background and Motivation <sec:motiv>

We first describe the executor, locks and workload, then demonstrate both problems (@fig:motiv).

== Cooperative executors
A tokio-style executor~@tokio runs $W$ worker threads. Each has a local run queue, and there is one shared injector queue. An idle worker steals half of a random peer's queue. tokio adds two policies that matter here: a _LIFO slot_ (a task woken from a worker is polled right after the current poll, cannot be stolen, and is capped at 3 in a row) and a _cooperative budget_ of 128 units per poll, consumed by `tokio::sync::Mutex` acquires.

Our executor (§@sec:impl) is built from `async-task`~@asynctask and `crossbeam-deque`~@crossbeam. It has per-worker FIFO queues, an injector and a single run-next slot. Every 31 polls a worker drains up to $l e n \/ W+1$ injector tasks (tokio's share rule). In our workload every worker hosts an always-runnable bystander, so steal-when-idle never fires. We therefore add a rate-limited _balancing steal_: every $b$ polls, a busy worker takes half of a random peer's queue. Runs use $b=31$ ("b31") or no balancing ("b0").

== Locks
We use the taxonomy of CES~@ces. Under _dispatch_ (`dispatch`; tokio, Kotlin, Boost), unlock enqueues the next waiter and the owner continues. Under _inline_, unlock resumes the waiter recursively. Under _CES_ (`ces`), unlock reschedules the owner remotely and resumes the head waiter inline. In _FC_ (`fc`)~@fc, clients publish closures, and whoever wins a `try_lock` runs a pass of up to $H$ of them. _FC-PQ_ (`fcpq`) is FC that serves the lowest cumulative charged usage first.

== Workload and metrics
The protected structure is a `BTreeMap<u64,u64>` with 65 536 keys. A critical section (CS) is one insert plus a TSC-timed spin: 1 000 cycles for light clients and 8× for heavy clients, half the clients each. Under _sustained_ load 64 clients do 4 000 cycles of parallel work between operations; under _bursty_ load 16 clients do 32 000 cycles. $W$ bystander tasks spin 1 000 cycles and then yield. We use two Jain indices~@jain, $J=(sum x_i)^2 \/ (n sum x_i^2)$: _service Jain_ over per-client CS cycles and _burden Jain_ over per-worker combining cycles. The _combiner-worker bystander p99_ is the p99 schedule-to-poll delay on the worker with the most combining cycles, against the maximum p99 of the other workers. A bystander first polled after the window is _censored_ ("cens." in the tables): it never ran.

#fig("figures/fig-motivation.svg", wide: true)[Both problems at $W=8$, heavy 8×. (a) Share of combining cycles per worker, workers sorted by share, sustained b0; the legend gives burden Jain. (b) p99 bystander schedule-to-poll delay on the combiner worker (filled) and the maximum over the other workers (open), same runs; CES's combiner-worker bystander is never polled in the 2~s window. (c) Per-client CS cycles over the mean, sustained b31; dashed: FIFO theory (equal operations per client at `dispatch`'s measured costs); the legend gives service Jain. `ces-k64`, `ces-k64-home`, `fc-remote` and `fcpq-h16-home-c16` are the policies of §@sec:design. All runs are from the 09-28 window except `fcpq-h16-home-c16` (hollow, 09-29). Markers and bars are medians of 3 repeats; error bars span [min, max].] <fig:motiv>

== Both problems, measured
@fig:motiv shows both problems at 8 workers. The full matrix covers $W in {4,8,16}$, heavy $in {1,8}$ and $b in {0,31}$ (432 runs); @tab:motiv in the appendix lists its $W=8$, heavy-8× cells.

#par(first-line-indent: 0pt)[*CES concentrates all combining on one worker.* In every saturated sustained cell CES has burden Jain exactly $1\/W$ (0.125 at 8 workers, 0.062 at 16), with or without balancing (@fig:motiv a). The median chain is 819 200 inline hand-offs: one chain lasts the whole window. Without balancing, CES traps what sits in the combiner's queue: 5 clients (range 5--6 across repeats) and the bystander starve for 2 s (@fig:motiv b), and service Jain drops to 0.574#rng[0.557][0.627]. With balancing, the combiner's bystander is stolen during warm-up and never returns; the burden shows up as eviction, not delay. Whether a chain ever ends is an executor property. An earlier executor version drained one injector task per 31 polls, which at $W <= 8$ let the wait queue empty periodically: burden Jain was 0.96 at 8 workers but 0.06 at 16.]

#par(first-line-indent: 0pt)[*FC without a yield hands the lock to one client.* Without balancing, the FC combiner's `run` completes synchronously, the waiters it served are woken into its own worker's queue, and the client loop never returns `Pending`. The same task re-publishes and re-wins the election forever: service Jain is $0.016=1\/64$ and 63 clients starve (bursty: 15 of 16). A yield after combining fixes the starvation but not placement: at b0, all clients converge on the combiner's worker (burden 1/W) and throughput is 0.235 instead of 0.390 Mops/s with balancing.]

#par(first-line-indent: 0pt)[*FIFO service is proportional to cost.* Under `dispatch` and `tokio::sync::Mutex` every light client receives 0.28× and every heavy client 1.71--1.72× the mean lock time, the FIFO prediction $2 C_L \/ (C_L+C_H)$ and $2 C_H \/ (C_L+C_H)$ (@fig:motiv c). The two-class model $J(x)=(1+x)^2 \/ (2(1+x^2))$, with $x$ the light:heavy per-client service ratio, predicts service Jain from each lock's measured costs to three digits for `dispatch`, CES, FC and `tokio::sync::Mutex` (@tab:fifo). Equal service would need 6.0--6.5 light operations per heavy one.]

#tbl("tables/tab-fifo.typ", colsep: 3pt)[FIFO service fairness, $W=8$, sustained, b31 (09-28 window). $"CS"_(L,H)$ are critical-section cycles per op (TSC). The model uses $x="L:H" dot "CS"_L \/ "CS"_H$. The last column is $"CS"_H$/$"CS"_L$.] <tab:fifo>

#par(first-line-indent: 0pt)[*Off-the-shelf async locks.* In tokio, only `tokio::sync::Mutex` serves every client (§@sec:q3). `async-lock`~@asynclock collapses to a single client in 11 of 12 matrix runs (63 of 64 or 15 of 16 clients starved); with the LIFO slot on it does so in 20 of 24 runs, with it off in 0 of 12. `std` and `parking_lot` mutexes block the worker thread: the first $W$ clients polled keep the $W$ workers for the whole run, so starved clients equal clients $- W$, and all $W$ bystanders starve.]

#par(first-line-indent: 0pt)[*Summary.* At their defaults, a delegation lock's two decisions put all combining on one worker, trap or starve the tasks behind it, and hand out lock time in proportion to cost. The executor cannot repair this: balancing evicts a trapped bystander rather than serving it, and whether a CES chain ends depends on the injector drain rate. A fix must move the combiner role, serve by usage, keep delegation's throughput, ask the executor only for wake placement, and bound every wait.]

// ---------------------------------------------------------------------------
= Design Goals <sec:goals>

We turn these requirements into goals, following SCL~@scl; each goal answers a failure of §@sec:motiv.
#[
#set enum(numbering: n => strong[G#n])
+ *Burden fairness.* Combining cycles are spread across workers (burden Jain $>= 0.9$). The p99 of a bystander on the combiner worker is within one histogram bucket of the p99 elsewhere. CES fails both.
+ *Service fairness.* Clients receive equal lock _time_, not equal operations. This requires usage accounting (service Jain $>= 0.95$ under 1:8 cost heterogeneity). FIFO locks reach 0.65--0.66.
+ *Work conservation.* Throughput is no worse than FIFO delegation. The fairness policy adds no idle lock time. Converging on one worker cost FC throughput at b0.
+ *Executor independence.* The lock asks the executor only for wake placement. It does not need priorities, preemption or scheduler callbacks, nor a lucky injector drain rate.
+ *Bounded wait.* No request waits more than a fixed number of passes, whatever its usage. FC without a yield fails it for 63 of 64 clients.
]
G2 and G3 conflict in one measurable way: fairness shifts the served mix toward cheap operations, and each operation carries a fixed lock cost. We quantify this conflict in §@sec:q5 rather than hide it in an ops/s number.

// ---------------------------------------------------------------------------
= Design <sec:design>

#figure(placement: top, scope: "parent", kind: image, contract,
  caption: [The executor contract and the two burden policies. (a) A lock may ask for each wake to be placed _Inline_ (the waking worker's run-next slot), _Remote_ (the shared injector) or _Home_ (the inbox of the worker that last polled the wakee). (b) CES resumes waiters inline on one worker; `ces-k64-home` ends the chain after $K=64$ hand-offs and hands ownership to the next waiter on its home worker, so the ex-combiner drains its own queue. (c) An FC combiner serves at most $H$ closures, wakes the served waiters remotely (`fc-remote`) and yields once after combining, so the tasks queued on its worker run before it competes again.]) <fig:contract>

This section presents the executor contract that G4 permits and the lock policy that meets each of G1, G2 and G5 (@fig:contract).

== Executor contract
A `Waker` can only enqueue its task, so it carries no placement. The executor therefore exposes a thread-local placement hint, which the schedule callback reads and resets (@fig:contract a). This hint is all a lock asks for (G4). `wake_with(Inline, w)` puts the task in this worker's run-next slot, which is polled as soon as the current poll returns, before the local queue and before stealing. The slot has no anti-starvation cap; that is deliberate, so that unbounded CES chains remain observable. `wake_with(Remote, w)` pushes the task to the injector and unparks one worker. `wake_with(Home, w)` pushes it to the inbox of the worker that last polled it. `reschedule_self_remote(cx)` marks the _current_ task `Remote`; `async-task` defers the schedule callback until the poll returns `Pending`. Tokio's LIFO slot is an implicit, capped `Inline` for every wake issued on a worker. The contract makes that choice explicit and per-wake, and adds no policy to the executor beyond what the lock requests.

== Burden: CES chain bound and break placement
A CES chain that never ends violates G1 and G5. The fix has two parts: a bound that ends the chain, and a placement that starts the next one elsewhere. A _chain_ is the sequence of inline resumes on one worker since that worker last polled anything else (@fig:contract b). `ces-k`$K$ ends a chain after $K$ hand-offs; `ces-t`$T$ ends it after $T$ cycles. At that point the unlocker hands ownership to the head waiter with a _break placement_ instead of resuming it inline, and continues, so the worker drains its own queue when the poll returns. With the default break placement, the woken owner lands in the ex-combiner's own queue and the next chain restarts there. The `-home` suffix sends it to its home worker's inbox, which moves the combiner role (§@sec:q1).

== Burden: FC yield, placement and pass limit
FC fails in two ways (§@sec:motiv): a combiner poll that never returns `Pending` starves every other client (G5), and wakes onto the combiner's worker pull all clients there (G1). Yield-after-combine fixes the first, wake placement the second. Every FC client owns one node, allocated once. A request writes a closure pointer and a trampoline into the node and pushes the node onto a Treiber stack. The client then tries the combiner flag. The winner drains the stack into the policy queue and runs at most $H$ closures (default 64). Each served waiter is marked `COMPLETE` (release/acquire) and woken with the lock's _wake placement_.

Losers return `Pending`; a waiter never spins. At the end of a pass, if the queue is empty, the combiner releases the flag and re-checks the stack (SeqCst on both sides). If its own request was served, it releases the flag and wakes the policy's next candidate, which re-runs the election. Otherwise it keeps combining.
_Yield-after-combine_ (default on) makes a poll that won the election return `Pending` once, having woken itself behind the waiters it just served (@fig:contract c). Preemption gives OS-thread FC the same effect. `-remote` and `-home` set the placement of every wake the lock issues.

== Service: usage-ordered FC-PQ
G2 needs usage accounting, which no FIFO lock has. FC-PQ keeps pending requests in a binary min-heap keyed by (cumulative charged cycles, arrival). The combiner charges each closure the `rdtscp` cycles around its execution, and the charge persists in the client's node. Two rules modify the key. A _newcomer_, meaning a client never served before, enters at the lock's running mean cost per request; zero, the minimum and the median are alternatives. The _starvation clamp_ enforces G5. At every pass start, an entry that has waited more than $c$ passes (default 8) has its key lowered to the current heap minimum until it is served. The accounting is untouched. This fixes a no-op in the reference implementation, which clamped after the pop.

The pass limit $H$ decides whether ordering matters at all. When $H >=$ the number of waiters, every pass admits everyone, ordering only permutes requests within a pass, and the share stays FIFO's.

#par(first-line-indent: 0pt)[*When does the clamp bind?* A clamp shorter than the wait that usage ordering would impose overrides the ordering, so G5 can defeat G2. Take $H=16$ with $N_h=32$ heavy clients. A heavy client's service interval is $2(1+"L:H")$ passes. Without the clamp, usage ordering settles where charged usage is equal, at an interval of $2 times 6.51=13.0$ passes. A clamp $c$ caps the interval at $(c+1)+0.28$ passes, where 0.28 is the return delay (parallel work plus wake). A clamp therefore binds only if $c+1.28<13.0$, i.e. $c <= 11$.]

Clamp 8 binds. Every heavy operation is then a clamp promotion: #PromotedCEight of requests are promoted, which equals the heavy share of operations $1\/(1+3.64)$. The interval is $(c+1)+0.28=9.28$ passes, and that gives L:H $=3.64$. Clamps of 12 and more do not bind: clamp 16 promotes at most 0.035 % of requests, all of them in the wait tail.

Service Jain follows from the two-class model with $x="L:H" dot "CS"_L \/ "CS"_H$. The model predicts 0.9395 at clamp 8 (measured 0.940) and 0.9970 at clamp 16 (measured 0.997). $J >= 0.95$ needs $x >= #XNinetyFive$, i.e. L:H $>= #LHNinetyFive$. By the binding rule that means $c >= 9$. For clamps 9--11 the model predicts $J approx 0.963$, 0.981 and 0.992. We did not run them.

Jain stops at 0.997 rather than 1 because the charge window includes the trampoline that moves the closure and its result. That is about 193 cycles per operation that the harness's CS timer does not see.

#par(first-line-indent: 0pt)[*Utilisation.* G2 and G3 meet in one identity. Let $macron(C)$ be the mean CS cycles per operation of the served mix and $o=(T-C S) \/ o p s$ the per-operation cycles not spent in a CS. Then lock utilisation is $C S \/ T = macron(C) \/ (macron(C)+o)$. The identity holds by definition; its content is empirical. For FC-family locks under sustained load, some combiner is busy 98.8--98.9 % of the window, so $o$ is the lock's per-op cost: in-pass administration plus hand-off gap. And $o$ is nearly independent of the clamp setting (#OCEight cycles at clamp 8, #OCSixteen at clamp 16). Fairness moves the mix toward cheap operations, which lowers $macron(C)$ (#CbarCEight to #CbarCSixteen cycles) and therefore lowers utilisation.]

== The obvious alternative: an actor
An application that wants G1 without a delegation lock would write a server task. `actor` spawns one server task per lock, lazily with the first request. Each server poll drains the request stack, serves up to 64 closures in FIFO order and yields. The server parks when there is nothing to serve. `actor-inline` changes three placements: the server is woken into the publisher's run-next slot, it yields `home`, and it wakes clients `remote`.

// ---------------------------------------------------------------------------
= Implementation <sec:impl>

The locks, executor and harness are written in Rust. @tab:loc gives the size of each part.

#par(first-line-indent: 0pt)[*Executor.* Tasks are `async-task` tasks. Each worker has a `crossbeam-deque` FIFO and a run-next slot; there is one shared `Injector`. Idle workers park through a SeqCst-fenced registration protocol that forbids lost wake-ups. Workers are pinned to distinct physical cores. *Requests.* Clients never allocate per request. Each owns one node, and the lock keeps all nodes alive so that a combiner never touches a freed node. `Run` futures hold the closure and the result slot inline. Completion is a release store of `COMPLETE`, and the owner reads it with an acquire load. All locks share one `rdtscp` counter with the harness.]

#par(first-line-indent: 0pt)[*Checks.* By the lock contract, no lock allocates per request in steady state: per-client nodes and in-place heaps or queues only. A counting-allocator probe in steady state measured 0~allocations per op for `fc` and `actor`. The executor queues allocate 0.012--0.016 per op for `fc-remote` and 0.016--0.025 for `actor-inline` (one `Injector` block per 63 remote wakes). `dispatch`, `ces` and `fcpq` were not probed #todo[allocation probe for `dispatch`/`ces`/`fcpq` not recorded]. The `actor` unit tests exercise mutual exclusion with overlap detection, the absence of lost wake-ups across idle transitions, and FIFO order. They pass in release builds and under ThreadSanitizer (0 reports). We have no TSan record for the other locks #todo[TSan runs for `dispatch`/`ces`/`fc`/`fcpq` not recorded]. `coro-bench --sanity` and `tokio-bench --sanity` (map length equals total ops) pass for all locks.]

#tbl("tables/tab-loc.typ")[Lines of Rust: non-blank, non-comment lines, with lines from the first `#[cfg(test)]` on counted as tests. Counted by `make_tables.py`.] <tab:loc>

// ---------------------------------------------------------------------------
= Evaluation <sec:eval>

In this section, we answer five questions, one per subsection.

#par(first-line-indent: 0pt)[*Setup.* The machine has 2× Intel Xeon Gold 6438M (32 cores per socket, SMT on, 128 logical CPUs), Linux 6.17.7, a 2.20 GHz TSC and `rustc` 1.100.0-nightly with `--release`. $W$ workers are pinned to logical CPUs $0..W-1$, one per physical core. Each run has a 200 ms warm-up and a 2 s window. Each configuration is repeated 3 times, with repeats as the outermost loop, and runs execute one at a time under a measurement lock. We report medians with [min, max] over the repeats, and we do not interpret differences inside the spread. Latencies are histogram bucket lower bounds at 6 % resolution. One unrelated single-threaded process ran during the 09-28 matrices. #todo[commit id of the measured binaries not recorded; binaries are identified by sha256 in FINDINGS.md]]

#par(first-line-indent: 0pt)[*Two measurement windows.* From 2026-09-29 00:07:54 UTC, CPUs 0--15 were capped at 3.0 GHz. On 09-28 they ran at about 3.69 GHz turbo. Spins are TSC-timed, so the cap slows only non-spin work: a light CS costs 1 365--1 376 instead of 1 303 cycles. Every table row and figure panel names its window (09-29 data are hollow where one axis shows both), and ratios are formed only within one window unless marked otherwise. The same binary paths re-run on 09-29 were 1.4--3.4 % slower than their 09-28 cells (`dispatch`, `ces-k64-home`, `fc-remote`), outside both spreads, and service Jain rose by 0.004--0.007. We attribute the drift to the cap [inference; the actor entry that measured it predates the diagnosis]. Cross-window ratios are therefore uncertain by about 3 %.]

== Q1: Does placement restore burden fairness? <sec:q1>

#fig("figures/fig-burden.svg", wide: true)[Burden fairness, phase 3, heavy 8× (09-28 window). Top: burden Jain over workers; bars are $W=8$, diamonds $W=16$ (measured for `ces` and `ces-k64-home` only); dashed: goal G1. Bottom: bystander p99 schedule-to-poll delay (µs, log scale) on the combiner worker (filled) and the maximum over the other workers (open). ▲: the combiner worker's bystander was never polled in the 2~s window (censored); ×: no bystander was left on the combiner worker (evicted by balancing). Medians of 3 repeats; error bars span [min, max]. Numbers and throughput: @tab:burden.] <fig:burden>

Yes, if the lock moves ownership off the combiner's worker, as `ces-k64-home` and `fc-remote` do. Without balancing, a chain bound alone or FC with home wakes do not (@fig:burden; @tab:burden lists the numbers and each variant's throughput relative to CES).

_A chain bound alone does nothing for burden at b0._ `ces-k64` keeps burden at 0.125 (left column of @fig:burden). The ex-combiner drains its own queue, the next acquirer is again one of its own clients, and the combiner's bystander p99 is 171 µs, the length of one 64-hand-off chain. The bound ends starvation but not concentration.

_The bound plus a home chain break fixes it._ `ces-k64-home` has burden Jain 0.993--0.999 in every cell (8 and 16 workers, sustained and bursty, b0 and b31). Its combiner-worker bystander p99 equals the others' (2.6--2.9 µs sustained; 44.7 and 14.9 µs bursty at 8 and 16 workers), and no task starves: in @fig:burden its filled and open markers coincide in all four columns. It costs 0.98--0.99× CES throughput sustained and 0.95--0.99× bursty.

_Bursty load needs no bound._ In bursty cells CES chains already end naturally (p50 12 hand-offs). The bound only trims the tail from a maximum of 152--191 to 64. The cycle budget (`ces-t64000-home`) breaks every $tilde.op$12 hand-offs even where the queue would have continued, and costs 5--8 %.

_For FC, `remote` beats `home`._ `fc-remote` removes the b0 one-worker convergence of `fc` (burden 0.981#rng[0.967][0.996] sustained, 1.000 bursty) at the balanced throughput level. `fc-home` reaches only 0.744 at b0 sustained. The election is sticky: the ex-combiner's own clients are woken locally and win the next `try_lock`. In bursty mode `home` is nevertheless faster (1.09--1.13× CES), because served waiters resume their parallel work on their own worker.

_Bystander delay did not exceed the other workers' under balancing._ Our pre-registered hypothesis was that the combiner-worker bystander p99 would exceed the other workers' p99 by more than one pass ($H times$ mean CS). It is refuted under balancing in all 12 bursty cells for all five combining variants: the two p99s are equal within one bucket (right column of @fig:burden), for example 44.7 against 44.7 µs for CES at 8 workers, with a 136.6 µs pass. Without balancing the effect is starvation, not delay.

== Q2: Does usage ordering restore service fairness? <sec:q2>

#tbl("tables/tab-service.typ", wide: true)[Service fairness, $W=8$, sustained, b31. Upper block: phase 3 (09-28). Lower block: same-window A/B and clamp sweep (09-29, 3.0 GHz cap); `-c`$N$ is `fcpq-h16-home` with starvation clamp $N$ (0 = off) and newcomer init _mean_. "model $J$" is the two-class model applied to the measured L:H and per-class costs. L:H, util and non-CS/op are medians (max spread 5 % of the median). util $=$ CS cycles / window. non-CS/op $=$ (window $-$ CS cycles) / ops; only for the FC-family rows, whose combiner is busy almost the whole window, is it the per-op lock cost $o$ of §@sec:design; for `dispatch` and `ces` it also contains lock-idle and hand-off time. Max wait is in combining passes. Do not compare rows across the two blocks.] <tab:service>

#fig("figures/fig-service.svg", wide: true)[Service fairness of FC-PQ, $W=8$, sustained, b31. (a) Pass limit $H$ (09-28 window): bars are measured service Jain, ticks the two-class model; dashed: goal G2. (b--f) Starvation-clamp sweep of `fcpq-h16-home`, newcomer init _mean_ (09-29 window): (b) service Jain with the model (ticks) and the confirmation cells (hollow; $W=8$ b0 and $W=16$ b31 were run at clamps 8 and 16 only); (c) light:heavy ops against the ratios the model needs for Jain 0.95 and 1; (d) throughput; (e) heavy-client run latency; (f) worst queue wait, with $c+1$ passes marked. Dashed green: same-window `fc-remote`. Medians of 3 repeats; error bars span [min, max].] <fig:service>

Yes, once the pass limit is below the backlog and the starvation clamp is too long to bind: `fcpq-h16-home-c16` reaches service Jain 0.997 with a 17-pass worst wait (@fig:service, @tab:service). @fig:service a shows that the pass limit decides whether usage ordering has any effect. `fcpq` with $H=64$ admits every waiter in every pass, so it reaches only 0.706. $H=8$ gives 0.763, $H=16$ 0.863, and $H=16$ with `home` wakes 0.932. The remaining FC-PQ knobs leave service Jain at or below `fcpq`'s level: pass budget `t16000` 0.661, rotation 0.708, combining credit 0.693, max-usage election 0.707 (@tab:service lists the main rows). None of them changes the admission set.

At $H=16$ with `home` wakes, the 8-pass clamp is the bound: L:H stays at 3.64, as §@sec:design predicts, below the #LHNinetyFive that Jain 0.95 needs (@fig:service c). Lengthening the clamp to 16 lifts service Jain to 0.997 at $W=8$ b31, $W=8$ b0 and $W=16$ b31 (@fig:service b; @tab:confirm in the appendix). L:H rises to 5.45--5.53, and ops throughput rises by 13 % (@fig:service d; 0.540→0.612 Mops/s, same window). Longer clamps change nothing further: clamp 32 and no clamp give the same Jain, L:H and throughput.

The price is heavy-client latency (@fig:service e). Heavy p50/p99 grow from 253/357 µs to 328/626 µs, and light p99 from 164 to 343 µs. The worst queue wait grows with the clamp (@fig:service f): 17 passes at clamp 16 and 33 at clamp 32. With the clamp off it is 106#rng[54][193] passes, with heavy maximum latencies of 1.5--4.3 ms. We therefore take `fcpq-h16-home-c16`: it has the fairness of clamp-off and a bounded worst case (G5).

Two settings had no effect. The newcomer rule fires on only 64 of about 1.2 M requests per run, all during warm-up when every usage is near 0, so mean, zero, min and median initialisation are indistinguishable. In bursty load every knob is inert (Jain 0.697 at $W=8$): with at most 16 waiters and 7.8 ops per pass, every pass serves everyone pending. Usage ordering can only act when the backlog exceeds $H$.

_Side effect on burden._ Clamp 16 also fixes a burden problem at b0, from 0.501#rng[0.500][0.544] to 0.970#rng[0.969][0.974]. Our explanation [inference]: under clamp 8 the pass-end hand-off almost always goes to a light client, and at b0 light clients live on 4 of the 8 workers.

== Q3: How do fair delegation locks compare with tokio's? <sec:q3>

#fig("figures/fig-xrt.svg", wide: true)[Throughput against tokio 1.53.1, relative to `tokio-mutex` (LIFO slot on) in the same cell; log scale (09-28 window; coro locks b31). Hatched: `tokio-mutex` with the LIFO slot disabled, as off/on within a throwaway `tokio_unstable` build whose LIFO-on runs are 0.96--1.00× the release binary. Black ticks on the delegation bars: the same throughput divided by LIFO-off `tokio-mutex`. Numbers above bars: clients that completed no operation in 2~s. `fcpq-h16-home` is clamp 8 and counts cheap light ops, so it is not a like-for-like throughput reference. Medians of 3 repeats; error bars span the numerator's [min, max].] <fig:xrt>

#tbl("tables/tab-xrt-thr.typ", wide: true, colsep: 3.5pt)[Throughput against tokio 1.53.1 (09-28 window; coro rows b31), the numbers of @fig:xrt. ×tm is the ratio to `tokio-mutex` in the same cell. *The bursty ratios of 4--5× are largely due to tokio's LIFO slot.*] <tab:xrt>

#tbl("tables/tab-xrt-fair.typ", wide: true, colsep: 3.5pt)[Service Jain and starved clients/bystanders for the runs of @tab:xrt. `std-mutex` and `parking-lot` are really $W$ pinned threads on a blocking mutex (see text).] <tab:xrtfair>

Under sustained load burden-fair delegation beats `tokio::sync::Mutex` by #(SusOnLo)--#(SusOnHi)×; under bursty load most of its advantage comes from tokio's LIFO slot. We ported the workload unchanged to tokio's multi-thread runtime (`tokio-bench`: same key stream, costs, histogram and pinning); @fig:xrt summarises the comparison, and @tab:xrt and @tab:xrtfair give the numbers.

#par(first-line-indent: 0pt)[*The executor is not a weak baseline.* Coro `dispatch` runs at 0.90--0.98× `tokio-mutex` (grey bars in @fig:xrt). The coop budget never forced a yield, and removing it (`-unconstrained`) moves throughput by 0--1 %. *Sustained load.* The burden-fair delegation locks (blue and green bars) starve no client and no bystander and keep FIFO-level service Jain (0.653--0.678, against 0.663--0.672 for `tokio-mutex`).]

#par(first-line-indent: 0pt)[*The LIFO-slot caveat.* Bursty ratios of #(BurOnLo)--#(BurOnHi)× come mostly from the slot. A mutex hand-off wake lands in the unlocker's LIFO slot, and the new owner waits there behind the unlocker's parallel work. Disabling the slot lifts `tokio-mutex` 3.4--3.7× in bursty mode and 1.08× in sustained mode (hatched bars; @tab:lifo). Against that configuration (black ticks), `ces-k64-home` and `fc-remote` run #(BurOffLo)--#(BurOffHi)× bursty and #(SusOffLo)--#(SusOffHi)× sustained. The advantage that holds under either tokio configuration is 1.5--1.6× sustained and about 1.2--1.4× bursty.]

#par(first-line-indent: 0pt)[*Blocking mutexes.* `std-mutex` and `parking-lot` beat every async lock at $W=8$ bursty (5.02× and 4.90×). In that cell they serve only 8 of the 16 clients and starve all bystanders (@fig:xrt, @tab:xrtfair), so the ratio is not like-for-like. Elsewhere the delegation locks beat `std-mutex` 1.80--3.28×. *Missing comparison.* The two runtimes were only measured together in the 09-28 window. `fcpq-h16-home-c16` (09-29) has no same-window tokio reference #todo[same-window tokio baseline for `fcpq-h16-home-c16`].]

== Q4: Is a server task enough? <sec:q4>

#fig("figures/fig-actor.svg")[Actor control: throughput relative to `fc-remote` in the same cell, against burden Jain. Shaded: the goal (burden Jain $>= 0.9$ at $>= 0.95×$ `fc-remote`). Marker shape gives the cell. At $W=8$ b31 all runs are from the 09-29 window. In the b0 and $W=16$ cells the references are stored 09-28 cells, so the actor ratios there (hollow) cross windows and understate the actor by up to about 3 %. Error bars span burden Jain's [min, max]. Numbers: @tab:actor.] <fig:actor>

No. Under sustained load each actor variant is either fast or burden-fair, never both (@fig:actor; @tab:actor in the appendix gives the numbers). Plain `actor` is burden-fair only because the executor's balancing steal keeps moving the server: burden Jain is 0.990 sustained and 0.999 bursty at $W=8$ b31. It runs 0.91× same-window `fc-remote` sustained and 0.54× bursty. Served clients are woken into the server's queue ahead of the yielded server, so each pass waits for their parallel work [inference]. At b0, `actor` is a one-worker system: burden 0.125, 0.59× sustained, 0.19× bursty, and a server-worker bystander p99 of 268--283 µs.

`actor-inline` restores throughput (1.04× sustained, 0.96× bursty at $W=8$ b31), but under sustained load the server never parks. A server that never parks is never re-placed by a wake, so only balancing steals move it. Burden is 0.179#rng[0.125][0.267] at $W=8$ and 0.612#rng[0.556][0.651] at $W=16$. At b0, its worker's bystander p99 is 156 µs, about one 64-request pass.

`ces-k64-home` and `fc-remote` reach the goal corner of @fig:actor because they move the combiner role by construction; a server task would need the same explicit migration.

== Q5: What does fairness cost? <sec:q5>

#fig("figures/fig-util.svg")[Lock utilisation (CS cycles / window) against the per-op cycles not spent in a critical section, $o$, for FC-family locks at $W=8$, sustained, b31. Curves: the identity util $= macron(C) \/ (macron(C)+o)$ for the FIFO mix ($macron(C)$ of same-window `fc-remote`), the mix at the Jain-0.95 bound, and the clamp-16 mix. Purple points are FC-PQ settings. Filled: 09-28; hollow: 09-29. Star: the $o$ that utilisation 0.80 needs at Jain 0.95. Error bars span [min, max].] <fig:util>

Service fairness costs lock utilisation, and the per-op lock cost $o$ accounts for all of the loss. Usage ordering alone costs little: at heavy ratio 1, FC-PQ's throughput is within 0--3 % of FC.

#par(first-line-indent: 0pt)[*Utilisation.* On 09-28, utilisation drops from #UtilFcOld (`fc`) to 0.725 (`fcpq-h16-home`); on 09-29, from #UtilFcRemoteAB (`fc-remote`) to #UtilCEight (clamp 8) and #UtilCSixteen (clamp 16), while a combiner is busy 98.8--98.9 % of the window (@fig:util). Each point sits on the curve of its own mix: FC-PQ moves left-to-right only a little ($o$) but drops to curves of cheaper mixes ($macron(C)$). The lock's cost is $o=#OCSixteen$ cycles/op, 96--97 % of it in-pass administration. Same-window `fc-remote` has $o=#OFcRemoteAB$, and the lowest $o$ of any fc-family sustained cell is `fc` on 09-28 at turbo clock, with $o=#OFcOld$.]

At the fairness bound $J=0.95$ (L:H $= #LHNinetyFive$), the mix costs $macron(C)=#CbarNinetyFive$ cycles, so utilisation 0.80 requires $o <= #ONeededEighty$ cycles/op (the star in @fig:util). That is 39 % below `fcpq-h16-home` and 25 % below `fc-remote`. Even $o=#OFcOld$ would give only #UtilAtOFcOld. The ops/s gain of FC-PQ is therefore partly an accounting effect: the lock does less work per second while completing more operations. Our first candidate for the cost [inference] is home wakes: on 09-28, `fcpq-h16` with default placement had 769 admin cycles/op against 1 005 for `-home`. We have not broken $o$ into drain, heap, rekey and wake costs #todo[breakdown of $o$ into drain / heap / rekey / home-wake cycles].

#par(first-line-indent: 0pt)[*Summary.* Placement restores burden fairness at CES throughput (Q1), and usage ordering restores service fairness once the pass limit is below the backlog and the clamp does not bind (Q2). Under sustained load fair delegation beats tokio's mutex in either configuration (Q3); a server task cannot match it without explicit migration (Q4). Service fairness is paid for in utilisation (Q5).]

// ---------------------------------------------------------------------------
= Discussion and Limitations <sec:discuss>

#par(first-line-indent: 0pt)[*Always-runnable bystanders.* Steal-when-idle never fires; our balancing steal substitutes for it and bounds the measurable bystander delay. Real bystanders are I/O-driven, and with idle gaps stealing could rescue trapped tasks and move a never-parking actor server. We have not tested this. *A synthetic workload.* One data structure plus spins, closed-loop with one await per iteration and no idle clients; there is no application benchmark. Late joiners and clients returning from idle were not exercised, and no newcomer rule handles idle return.]

#par(first-line-indent: 0pt)[*One machine, two clock windows.* Three repeats on one machine give min/max spreads, not confidence intervals. The clock cap splits the data into two windows (§@sec:eval); cross-window ratios are uncertain by about 3 %. *Our CES.* The CES paper links no code; `ces` implements its description~@ces, and every CES number here is for our implementation. *Future work.* I/O-driven bystanders; a remote-yield actor, i.e. explicit server migration; clamps 9--12, to trace the predicted Jain/latency frontier; reducing $o$ below 703 cycles/op, starting with home wakes; and real applications.]

// ---------------------------------------------------------------------------
= Related Work <sec:related>

#par(first-line-indent: 0pt)[*Scheduler-cooperative locks.* SCL and its user-space variant U-SCL~@scl name _scheduler subversion_ for OS threads, where lock usage rather than scheduler share decides CPU allocation, and fix it with usage accounting and lock-slice penalties. CFL~@cfl schedules lock occupation by cgroup share and priority, and is NUMA-aware. Our service-fairness goal is their idea (fair lock _time_), but in SCL and CFL each thread runs its own critical section; a delegation lock also decides whose worker pays.]

#par(first-line-indent: 0pt)[*Delegation locks.* FC~@fc is starvation-free and linearizable. It lists the number of consecutive combining rounds as a knob and does not measure the combiner's own delay. TCLocks~@tclocks bound combining batches at 1 024 waiters for "long-term fairness". They give no fairness metric and defer charging the waiter for combining. Neither measures a per-thread combiner share; we measure it and move it. The FC dispatcher poster~@fcdispatch rotates the dispatcher role, which mitigates burden, but does not measure the cost.]

#par(first-line-indent: 0pt)[*CES.* CES~@ces states that it is the first delegation-style lock for cooperatively scheduled tasks; the claim is theirs. It evaluates throughput only, at uniform CS cost, and notes qualitatively that the CS stays on one thread per resource. We build on its inline/remote exchange and add what a fairness analysis needs: chain bounds with break placement, the burden metric, and usage-ordered service.]

#par(first-line-indent: 0pt)[*Async runtimes and locks.* The tokio mutex~@tokio is a FIFO semaphore whose hand-off wake goes through the LIFO slot. Reading its source, this is a bounded inline-after-poll hand-off on the unlocker's worker [inference, consistent with the LIFO side check of §@sec:q3]. Its fairness scope is only the waiters~@tokio6049. `async-lock`~@asynclock relies on a 0.5 ms starvation hand-off, which never fires when the notified waiter is never polled.]

#par(first-line-indent: 0pt)[*Preemptive request scheduling.* Shinjuku~@shinjuku preempts every 5--15 µs to handle heterogeneous request costs and places contended locks out of scope; Perséphone's DARC~@persephone reserves cores per request type and assumes independent requests. Both handle cost heterogeneity in the request scheduler. We handle it inside the lock, where a cooperative executor has no preemption to fall back on.]

// ---------------------------------------------------------------------------
= Conclusion <sec:concl>

In this paper, we have shown that a delegation lock in a cooperative executor is a hidden scheduler whose two decisions, where the combiner runs and whom it serves, belong to the lock. With only wake-placement hints, a lock can spread combining (burden Jain 0.98--1.00, against $1\/W$) and share lock time equally (service Jain 0.997, against 0.65--0.66, with a 17-pass wait bound), and still run 1.5--1.6× `tokio::sync::Mutex` under sustained load; service fairness costs utilisation (0.84→0.68). Our hope is that runtimes will let locks choose where their wakes land, so that the scheduler hidden in each lock can cooperate with the one outside it.

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
