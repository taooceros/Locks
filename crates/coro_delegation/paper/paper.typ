#import "lib.typ": rng, fit
#import "tables/numbers.typ": *
#import "figures/contract.typ": contract

// A number or result the text wants but no FINDINGS.md entry or JSON provides.
#let todo(body) = text(fill: red, weight: "bold")[\[TODO: #body\]]

#let title = [Fair Locks for Coroutine Runtimes:\ Ordinary Wakers Buy Fairness, Executor Awareness Buys Speed]

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
#let tbl(file, caption, wide: false, colsep: 6pt, placement: top, size: 8pt) = figure(
  kind: table,
  placement: placement,
  scope: if wide { "parent" } else { "column" },
  caption: caption,
  {
    set text(size: size)
    set par(justify: false)
    set table(inset: (x: colsep, y: if size < 8pt { 1.2pt } else { 1.6pt }))
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
    In a coroutine runtime a lock decides _who_ gets ownership next, and, if its unlock is an explicit `await`, _when_ the releaser steps aside. We show that a lock is therefore a hidden scheduler that can subvert the executor: FIFO locks give lock time in proportion to critical-section cost (service Jain 0.65--0.67 for a 1:8 cost mix), and delegation locks concentrate combining on one worker (burden Jain $1\/W$), which lock-side wake placement repairs. We then separate policy from mechanism. A usage-ordered queue restores service fairness with _ordinary wakers only_: our `co-pq` reaches service Jain #(CoPqJainMin) or better in all #(CoPqCells) measured cells with no starved client. It is not cheap. At the sustained fair mix it spends #(OSusSpinCoPq) non-critical-section cycles per operation (spin harness), #(PqOverFcpqOSusSpin)× a usage-ordered combining lock (#(OSusSpinFcpq)). At the FIFO mix an inline hand-off (`co-fifo`, #(OSusSpinCoFifo)) runs within 5 % of the ops/s of a combining lock (#(OSusSpinFc)) and at #(DispOverCoFifoOSusSpin)× lower cost than an ordinary-waker hand-off (`dispatch`, #(OSusSpinDisp)). Executor awareness, not the fairness policy, buys speed, and the ordinary-wake cost depends on the runtime: `tokio::sync::Mutex` runs at #(OSusYieldTok) cycles per operation when clients yield after unlocking, #(TokOverDispThrSusYield)× the ops/s of our `dispatch`. The critical section is a TSC-timed spin, on one machine, with clients that never leave.
  ]
]

// ---------------------------------------------------------------------------
= Introduction <sec:intro>

Coroutines are back in wide use: many modern languages support cooperative multitasking~@ces. Runtimes such as tokio~@tokio multiplex many tasks onto a few worker threads. A task runs until it returns `Pending`, and workers balance load by stealing. Fairness guarantees are weak and cover only runnable tasks, or only a lock's waiters~@tokio6049. A lock in such a runtime is usually an _async mutex_: a waiter suspends, and the unlocker wakes it through the scheduler. CES~@ces observed that this puts a scheduler round trip on the critical path and resumes the next waiter inline instead, which makes it a delegation lock, like flat combining (FC)~@fc and TCLocks~@tclocks.

Patel et al.~@scl showed that ordinary locks _subvert_ the OS scheduler: lock hold time, not the scheduler's share, decides who gets CPU time. A cooperative executor is exposed to more than that, because it cannot preempt, and because a lock also decides _where_ work runs. We call this _executor subversion_ and treat every lock as a _hidden scheduler_ with two decisions. _Who_ gets ownership next is its grant order. _When_ the releaser steps aside, and where the next owner runs, is its unlock path. If unlock is an explicit `await` (`g.unlock().await`), the lock can make the second decision without a callback into the executor.

As a simple example, take $W=8$ workers, 64 clients, half with 8× the critical-section (CS) cost of the other half, and one always-runnable bystander per worker (@fig:motiv). Under strict FIFO every client gets equal operations, hence lock time in proportion to cost: the predicted service Jain index is 0.661 for `dispatch` (from its measured 1 391 and 8 403 cycles per op, @tab:fifo), and `dispatch`, CES, FC and `tokio::sync::Mutex` all reproduce their own prediction to within 0.001 (@fig:motiv a). A usage-ordered lock, `co-pq`, gives every client equal lock time (hollow markers in @fig:motiv a). Two more forms are structural: `async-lock` hands the lock to a single client in #(AsyncMonoRuns) of #(AsyncRuns) runs, and `std` and `parking_lot` mutexes seize the first $W$ clients' workers (@fig:motiv b); and a delegation lock concentrates combining on one worker (@fig:motiv c, d).

#par(first-line-indent: 0pt)[*The crux.* What does a fair lock need from the executor, and what does executor awareness buy on top of it?]

In this paper, we separate the policy that restores service fairness from the mechanisms that carry it. The policy is a usage-ordered queue (lowest cumulative charged CS cycles first, with a starvation clamp). It is lock-agnostic: we run it behind an ordinary hand-off (`co-pq`, `dispatch-pq`) and inside a combining lock (FC-PQ). The mechanisms are the executor-aware tricks: a placement argument on wakes (inline, remote, home), an explicit async unlock that steps the releaser aside, and delegation.

We find that fairness needs only ordinary wakers: `co-pq` wakes its chosen waiter with the plain `Waker`, and reaches service Jain #(CoPqJainMin) or better, with L:H served operations #(CoPqLHLo)--#(CoPqLHHi), in every one of #(CoPqCells) cells (W8 and W16, sustained and bursty, spin and yield). We also find that speed needs more. At one fair mix (L:H $approx$ 5.3) `co-pq` costs #(OSusSpinCoPq) cycles per operation against #(OSusSpinFcpq) for FC-PQ; at the FIFO mix an inline hand-off with a step-aside costs #(OSusSpinCoFifo) against #(OSusSpinDisp) for an ordinary-waker hand-off. This contradicts our expectation that an ordinary wake plus an async step-aside would land near the executor-aware locks. It also qualifies an earlier version of this paper, which attributed the hand-off cost to the executor and the fast case to delegation: part of that cost was manufactured by our harness, and the rest depends on wake placement, not on delegation (§@sec:discuss).

Our contributions are:
+ Measurements of executor subversion (FIFO service proportional to cost, combiner burden, monopoly and seizure); FIFO service Jain follows from per-class costs to three digits (§@sec:motiv).
+ A lock-agnostic usage-ordered policy with a rule for when its starvation clamp binds, in passes or in hand-offs (predicted Jain 0.9395, measured 0.940), and coroutine-style locks whose async unlock is the point where the lock steps the releaser aside (§@sec:design).
+ An evaluation from one frozen binary in one window, reported by the per-op cost $o$ at a stated mix: fairness with ordinary wakers, and what inline hand-off and combining buy on top, under both harness modes (§@sec:eval).

#par(first-line-indent: (amount: 1em, all: true))[The rest of this paper is organized as follows. §@sec:motiv demonstrates the forms of subversion and §@sec:goals turns them into goals. §@sec:design and §@sec:impl present the policy, the locks and the executor contract, and §@sec:eval evaluates them. §@sec:discuss states limitations and withdrawn claims; §@sec:related–§@sec:concl cover related work and conclusions.]

// ---------------------------------------------------------------------------
= Background and Motivation <sec:motiv>

We first describe the executor, locks and workload, then demonstrate the forms of subversion (@fig:motiv).

== Cooperative executors
A tokio-style executor~@tokio runs $W$ worker threads. Each has a local run queue, and there is one shared injector queue. An idle worker steals half of a random peer's queue. tokio adds two policies that matter here: a _LIFO slot_ (a task woken from a worker is polled right after the current poll, cannot be stolen, and is capped at 3 in a row) and a _cooperative budget_ of 128 units per poll, consumed by `tokio::sync::Mutex` acquires.

Our executor (§@sec:impl) is built from `async-task`~@asynctask and `crossbeam-deque`~@crossbeam. It has per-worker FIFO queues, an injector and a single run-next slot. An ordinary `Waker` pushes the woken task to the back of the waking worker's local queue. Every 31 polls a worker drains up to $l e n \/ W+1$ injector tasks (tokio's share rule). In our workload every worker hosts an always-runnable bystander, so steal-when-idle never fires. We therefore add a rate-limited _balancing steal_: every $b$ polls, a busy worker takes half of a random peer's queue. Runs use $b=31$ ("b31") or no balancing ("b0"). The executor has no timer.

== Locks
We use the taxonomy of CES~@ces. Under _dispatch_ (`dispatch`; tokio, Kotlin, Boost), unlock enqueues the next waiter and the owner continues. Under _inline_, unlock resumes the waiter recursively. Under _CES_ (`ces`), unlock reschedules the owner remotely and resumes the head waiter inline. In _FC_ (`fc`)~@fc, clients publish closures, and whoever wins a `try_lock` runs a pass of up to $H$ of them. `dispatch-pq`, `co-pq` and FC-PQ (`fcpq`) use a usage-ordered queue (§@sec:policy). The coroutine-style locks `co-fifo` and `co-pq` (§@sec:design) run the critical section as the task's own code, between `lock().await` and `unlock().await`.

== Workload and metrics
The protected structure is a `BTreeMap<u64,u64>` with 65 536 keys. A critical section (CS) is one insert plus a _TSC-timed spin_: 1 000 cycles for light clients and 8× for heavy clients, half the clients each. The spin is unaffected by cache state, so no lock can change a CS's cost: the locality argument for delegation is excluded by construction, and differences between locks are scheduling differences. Under _sustained_ load 64 clients do 4 000 cycles of parallel work between operations; under _bursty_ load 16 clients do 32 000 cycles. The parallel work runs in one of two harness modes. In `spin` it is spun in the poll that released the lock; in `yield` the client first calls `yield_now().await`, as code that awaits I/O after unlocking would. $W$ bystander tasks spin 1 000 cycles and then yield. We use two Jain indices~@jain, $J=(sum x_i)^2 \/ (n sum x_i^2)$: _service Jain_ over per-client CS cycles and _burden Jain_ over per-worker burden. For FC-family locks burden is combining cycles; for `ces` and `co-*` it is foreign-CS cycles, the CS cycles a worker runs for requests homed elsewhere. The two definitions are not the same quantity. The _combiner-worker bystander p99_ is the p99 schedule-to-poll delay on the worker with the most combining cycles, against the maximum p99 of the other workers.

#par(first-line-indent: 0pt)[*Efficiency, at a stated mix.* Throughput and utilisation are not comparable across locks that serve different mixes: usage ordering shifts the served light:heavy ratio (L:H) from 1:1 to about 5.3:1 and halves the mean CS cost. We therefore compare efficiency by $o=(T-C S)\/o p s$, the window $T$ minus the CS cycles, per operation, and always state the mix. $o$ is a residual elapsed time, not a count of lock instructions: it includes hand-off, wake and queue wait, and, in bursty cells, lock-idle time. Ops/s ratios are used only between cells with the same mix (L:H within 4 %).]

#fig("figures/fig-motivation.svg", wide: true)[Subversion, heavy 8×. (a) Per-client CS time over the mean, $W=8$ sustained b31: FIFO (`dispatch`, `tokio-mutex`, 09-28) and the usage-ordered `co-pq` (hollow, 09-30 window, spin); dashed: FIFO theory at `dispatch`'s measured costs. (b) Monopoly and worker seizure (tokio runtime, spin harness, 09-28): share of clients (bars) and bystanders (◇) that completed nothing in 2~s. (c) Combiner burden: share of combining cycles per worker, sorted, $W=8$ sustained b0. (d) Bystander p99 schedule-to-poll delay on the combiner worker (filled) and the maximum elsewhere (open), same runs. (c, d) are from the 09-28 window. Medians of 3 repeats; error bars span [min, max].] <fig:motiv>

== Three forms of subversion, measured <sec:forms>
@fig:motiv summarizes the forms. The coro matrix covers $W in {4,8,16}$, heavy $in {1,8}$ and $b in {0,31}$ (432 runs, 09-28); the tokio runs cover $W in {8,16}$, sustained and bursty. The first two forms decide _who_ runs next, the third _where_.

#par(first-line-indent: 0pt)[*FIFO service is proportional to cost (every FIFO lock).* Under `dispatch` and `tokio::sync::Mutex` every light client receives 0.28× and every heavy client 1.71--1.72× the mean lock time, the FIFO prediction $2 C_L \/ (C_L+C_H)$ and $2 C_H \/ (C_L+C_H)$ (@fig:motiv a). The two-class model $J(x)=(1+x)^2 \/ (2(1+x^2))$, with $x$ the light:heavy per-client service ratio, predicts service Jain from each lock's measured costs to three digits for `dispatch`, CES, FC and `tokio::sync::Mutex` (@tab:fifo). Equal service would need 6.0--6.5 light operations per heavy one.]

#tbl("tables/tab-fifo.typ", colsep: 3pt)[FIFO service fairness, $W=8$, sustained, b31 (09-28 window). $"CS"_(L,H)$ are critical-section cycles per op (TSC). The model uses $x="L:H" dot "CS"_L \/ "CS"_H$. The last column is $"CS"_H$/$"CS"_L$.] <tab:fifo>

#par(first-line-indent: 0pt)[*Monopoly and seizure (`async-lock`, blocking mutexes).* `async-lock`~@asynclock collapses to a single client in #(AsyncMonoRuns) of #(AsyncRuns) tokio runs: 63 of 64 or 15 of 16 clients starve (@fig:motiv b). `std` and `parking_lot` mutexes block the worker thread, so the first $W$ clients polled keep the $W$ workers for the whole run: starved clients equal clients $- W$, and all $W$ bystanders starve.]

#par(first-line-indent: 0pt)[*Combiner burden (delegation only).* In every saturated sustained cell CES has burden Jain exactly $1\/W$ (0.125 at 8 workers, 0.062 at 16), with or without balancing (@fig:motiv c): the median chain is 819 200 inline hand-offs, one chain for the whole window. Without balancing, CES traps what sits in the combiner's queue: 5 clients (1--7 across repeats) and the bystander starve for 2 s (@fig:motiv d), and service Jain drops to 0.617#rng[0.574][0.646]. With balancing, the bystander is stolen during warm-up and never returns, so balancing and always-runnable bystanders are load-bearing choices of this workload. Whether a chain ends is an executor property: an earlier executor that drained one injector task per 31 polls let the queue empty at $W <= 8$ (burden Jain 0.96 at 8 workers, 0.06 at 16). FC without a yield is worse: without balancing, served waiters are woken into the combiner's own queue and the client loop never returns `Pending`, so one task re-wins the election forever (service Jain $0.016=1\/64$). A yield after combining ends the starvation, but at b0 all clients still converge on the combiner's worker (burden $1\/W$).]

#par(first-line-indent: 0pt)[*Hand-off delay is not a form of subversion we can claim.* An earlier version listed tokio's LIFO slot as a fourth form, because `tokio::sync::Mutex` was 3.4--3.7× slower in bursty mode with the slot on; a same-session rerun shows that the delay comes from the spin harness, not from the lock (§@sec:q5).]

#par(first-line-indent: 0pt)[*Summary.* At their defaults, locks in a cooperative executor hand out lock time in proportion to cost or to luck, and decide where work runs by blocking workers or by concentrating combining on one worker; only the last is specific to delegation. The executor cannot repair them: it cannot preempt a blocked or combining worker, and balancing evicts a trapped bystander rather than serving it.]

// ---------------------------------------------------------------------------
= Design Goals <sec:goals>

We turn these requirements into goals, following SCL~@scl; each goal answers a form of §@sec:motiv.
#[
#set enum(numbering: n => strong[G#n])
+ *Service fairness.* Clients that contend continuously receive equal lock _time_, not equal operations: service Jain $>= 0.95$ under 1:8 cost heterogeneity, which needs usage accounting (FIFO: 0.65--0.67). We do not claim it for clients that come and go (§@sec:discuss).
+ *Bounded wait.* No request waits more than a fixed number of lock decisions, whatever its usage (`async-lock` and FC without a yield fail it).
+ *Worker fairness.* No worker is seized by lock work: burden Jain $>= 0.9$, and the combiner worker's bystander p99 within one histogram bucket of the others' (blocking mutexes and CES fail it).
+ *Low per-op cost.* The fair policy adds little lock time per operation: $o$ at a stated mix.
+ *Executor independence.* The lock asks the executor for as little as possible: ordinary wakers, or a placement argument on wakes; no priorities, preemption, scheduler callbacks or lucky injector drain rate.
]
G1, G4 and G5 pull apart: what an ordinary waker costs is a property of the runtime that places it. We quantify this in §@sec:q2 and §@sec:q3.

// ---------------------------------------------------------------------------
= Design <sec:design>

#figure(placement: top, scope: "parent", kind: image, contract,
  caption: [The executor contract and the two burden policies. (a) A lock may ask for each wake to be placed _Inline_ (the waking worker's run-next slot), _Remote_ (the shared injector) or _Home_ (the inbox of the worker that last polled the wakee); an ordinary `Waker` asks for nothing, and `co-pq` uses only that. (b) CES resumes waiters inline on one worker; `ces-k64-home` ends the chain after $K=64$ hand-offs and hands ownership to the next waiter on its home worker, so the ex-combiner drains its own queue. (c) An FC combiner serves at most $H$ closures, wakes the served waiters remotely (`fc-remote`) and yields once after combining, so the tasks queued on its worker run before it competes again.]) <fig:contract>

This section presents the two decisions a coroutine lock makes, the executor contract they need, a usage-ordered policy that meets G1 and G2 for any lock, the locks, and the placement policies that meet G3 for delegation locks (@fig:contract).

== Two decisions: who, and when the releaser steps aside <sec:api>
In coroutine style the critical section is the task's own code: `let mut g = h.lock().await; /* CS */ g.unlock().await;`. _Who_ runs next is the grant order: the lock picks the successor while it holds its queue spinlock and grants ownership before waking it. _When_ the releaser steps aside is decided in `unlock().await`: a lock may wake the successor and then return `Pending` once, so the releaser re-queues behind whatever the executor runs first. `drop(g)` must stay correct as a synchronous release, without the step-aside, and is not the measured path. Two locks implement this (`co_mutex.rs`). `co-fifo` keeps a FIFO of waiters: `unlock().await` grants the head, wakes it _inline_ on the releaser's worker and steps the releaser aside to the injector. Chains of inline hand-offs are bounded (K=64) and broken with a `home` wake (§@sec:burden). `co-pq` selects the minimum-usage waiter from the `UsageQueue` (§@sec:policy) and wakes it with a plain `wake_by_ref()`: no placement hint, no inline hand-off. Its async release self-wakes and returns `Pending` once, also without hints. The runtime chooses where everything runs.

== Executor contract
A `Waker` can only enqueue its task, so it carries no placement. Locks that want more use a thread-local placement hint, which the schedule callback reads and resets (@fig:contract a). `wake_with(Inline, w)` puts the task in this worker's run-next slot, polled as soon as the current poll returns; the slot has no anti-starvation cap, so unbounded CES chains remain observable. `wake_with(Remote, w)` pushes the task to the injector and unparks one worker; `wake_with(Home, w)` pushes it to the inbox of the worker that last polled it. `reschedule_self_remote(cx)` marks the _current_ task `Remote`, applied when its poll returns `Pending`. Tokio's LIFO slot is an implicit, capped `Inline` for every wake issued on a worker; the contract makes that choice explicit and per-wake. `co-pq` needs none of it; `co-fifo`, `ces`, `fc-remote` and `dispatch-pq-home` use it.

== A lock-agnostic usage-ordered policy <sec:policy>
G1 needs usage accounting, which no FIFO lock has. Our policy is a queue, `UsageQueue`, generic over the waiter type and shared unchanged by the fair locks. It keeps waiting requests in a binary min-heap keyed by (cumulative charged cycles, arrival). The lock charges each CS the `rdtscp` cycles around it (for `co-pq`: from `lock()` returning to `unlock()`), and the charge persists in the client's node. Usage is cumulative and never decays, so the policy is fair among clients that contend continuously and rewards an absent client on return (§@sec:discuss). Two rules modify the key. A _newcomer_, meaning a client never served before, enters at the lock's running mean cost per request. The _starvation clamp_ enforces G2. At every _tick_, an entry that has waited more than $c$ ticks has its key lowered to the current heap minimum until it is served; the accounting is untouched. The tick is the lock's unit of decision: a combining pass in FC-PQ, a hand-off in `dispatch-pq` and `co-pq`.

#par(first-line-indent: 0pt)[*When does the clamp bind?* A clamp shorter than the wait that usage ordering would impose overrides the ordering, so G2 can defeat G1. Take FC-PQ with pass limit $H=16$ and $N_h=32$ heavy clients. A heavy client's service interval is $2(1+"L:H")$ passes. Without the clamp, usage ordering settles where charged usage is equal, at an interval of $2 times 6.51=13.0$ passes. A clamp $c$ caps the interval at $(c+1)+0.28$ passes, where 0.28 is the return delay (parallel work plus wake). A clamp therefore binds only if $c+1.28<13.0$, i.e. $c <= 11$. Clamp 8 binds: #PromotedCEight of requests are promoted, and the interval $(c+1)+0.28=9.28$ passes gives L:H $=3.64$ (@tab:clamp).]

The same rule, counted in hand-offs, sets the clamp of `dispatch-pq` and `co-pq`. Under usage order a light request waits 36 hand-offs (p50), and a heavy client is served once per round of 32 heavy and $32 times 5.84$ light operations, about 219 hand-offs. A clamp of 8 or 16 hand-offs is shorter than even the light wait, so it promotes every request: promoted keys collapse to the running minimum, ties go by arrival, and service is FIFO. A hand-off clamp must be about 16× looser than a 16-op-pass clamp; the default is 256. Service Jain follows from the two-class model with $x="L:H" dot "CS"_L \/ "CS"_H$: it predicts 0.9395 at clamp 8 (measured 0.940) and 0.9970 at clamp 16 (measured 0.997). $J >= 0.95$ needs L:H $>= #LHNinetyFive$ ($x >= #XNinetyFive$), i.e. $c >= 9$.

== Two more realisations: hand-off and combining <sec:realise>
#par(first-line-indent: 0pt)[*Hand-off: `dispatch-pq`.* The closure-style hand-off mutex: an uncontended acquire is one CAS; release pops the minimum under a TTAS spinlock, applies the clamp, keeps the lock held and wakes the grantee with the lock's placement. `co-pq` is its coroutine-style sibling with no placement.]

#par(first-line-indent: 0pt)[*Combining: FC-PQ.* Every FC client owns one node. A request pushes a closure onto a Treiber stack and tries the combiner flag. The winner drains the stack into the policy queue (FIFO for `fc`, `UsageQueue` for FC-PQ) and runs at most $H$ closures (default 64), marking each served waiter `COMPLETE` and waking it with the lock's _wake placement_. Losers return `Pending`; a waiter never spins. The pass limit decides whether ordering matters at all: when $H >=$ the number of waiters, every pass admits everyone and the share stays FIFO's. The combiner runs the chosen closure itself and wakes the client only after its CS has run.]

== Combiner burden: bounds and placement <sec:burden>
#par(first-line-indent: 0pt)[*CES chain bound and break placement.* A CES chain that never ends violates G2 and G3. The fix has two parts: a bound that ends the chain, and a placement that starts the next one elsewhere. A _chain_ is the sequence of inline resumes on one worker since that worker last polled anything else (@fig:contract b). `ces-k`$K$ ends a chain after $K$ hand-offs; `ces-t`$T$ ends it after $T$ cycles. At that point the unlocker hands ownership to the head waiter with a _break placement_ instead of resuming it inline, and continues, so the worker drains its own queue when the poll returns. With the default break placement, the woken owner lands in the ex-combiner's own queue and the next chain restarts there. The `-home` suffix sends it to its home worker's inbox, which moves the combiner role (§@sec:q1). `co-fifo` uses the same bound and break.]

#par(first-line-indent: 0pt)[*FC yield and placement.* FC fails in two ways (§@sec:motiv): a combiner poll that never returns `Pending` starves every other client (G2), and wakes onto the combiner's worker pull all clients there (G3). _Yield-after-combine_ (default on) fixes the first: a poll that won the election returns `Pending` once, having woken itself behind the waiters it just served (@fig:contract c). Wake placement fixes the second: `-remote` and `-home` set the placement of every wake the lock issues.]

// ---------------------------------------------------------------------------
= Implementation <sec:impl>

The locks, executor and harness are written in Rust. @tab:loc in the appendix gives the size of each part.

#par(first-line-indent: 0pt)[*Executor and requests.* Tasks are `async-task` tasks; each worker has a `crossbeam-deque` FIFO and a run-next slot, and there is one shared `Injector`. Idle workers park through a SeqCst-fenced registration protocol that forbids lost wake-ups. Clients never allocate per request: each owns one node or waiter, which the lock keeps alive. All locks share one `rdtscp` counter with the harness.]

#par(first-line-indent: 0pt)[*Checks.* A counting-allocator probe in steady state measured 0~allocations per op for `fc`, `actor` and `dispatch-pq` (default wakes); the executor queues add 0.012--0.016 per op for `fc-remote` and 0.0159 for `dispatch-pq` with home or remote wakes (one `Injector` block per 63 pushes). `dispatch`, `ces`, `fcpq`, `co-fifo` and `co-pq` were not probed #todo[allocation probe for `dispatch`/`ces`/`fcpq`/`co-fifo`/`co-pq` not recorded]. The `dispatch-pq` tests (minimum-usage grant order, the clamp in hand-offs, dropping a queued or granted request, both sides of the release/enqueue race, mutual exclusion on the real executor) pass in release builds and under ThreadSanitizer (0 reports). We have no TSan record for the other locks #todo[TSan runs for `dispatch`/`ces`/`fc`/`fcpq`/`co-fifo`/`co-pq` not recorded]. `coro-bench --sanity` and `tokio-bench --sanity` (map length equals total ops) pass for all locks. The 43 release tests passed on the ordinary-waker `co-pq` change, before the `co2` runs (FINDINGS.md).]

// ---------------------------------------------------------------------------
= Evaluation <sec:eval>

In this section, we answer five questions, one per subsection.

#par(first-line-indent: 0pt)[*Setup.* 2× Intel Xeon Gold 6438M (32 cores per socket, SMT on), Linux 6.17.7, a 2.20 GHz TSC, `rustc` 1.100.0-nightly `--release`. $W$ workers are pinned to logical CPUs $0..W-1$, one per physical core. Each run has a 200 ms warm-up and a 2 s window; each configuration is repeated 3 times, repeats outermost, one run at a time under a measurement lock. We report medians and do not interpret differences inside the [min, max] spread (in @tab:co2all). Latencies are histogram bucket lower bounds at 6 % resolution. #todo[commit id of the measured binaries not recorded; binaries are identified by sha256 in FINDINGS.md]]

#par(first-line-indent: 0pt)[*Three measurement windows.* From 2026-09-29 00:07:54 UTC, CPUs 0--15 were capped at 3.0 GHz (09-28: about 3.69 GHz turbo). The 09-30 window is a later session under the same cap. Spins are TSC-timed, so the cap slows only non-spin work. Every table row and figure panel names its window, later windows are hollow where one axis shows several, and ratios are formed within one window. All current results on `co-pq`, the references beside it and `tokio::sync::Mutex` come from one window (09-30, `co2-*`): one frozen `coro-bench` and one frozen `tokio-bench`, 120 and 12 runs, sha256 in FINDINGS.md. Binaries re-run across windows were 1.4--3.4 % slower than their earlier cells [inference: the cap], so cross-window ratios are uncertain by about 3 % and we make none.]

== Q1: Does placement restore burden fairness? <sec:q1>

#fig("figures/fig-burden.svg", wide: true)[Burden fairness, phase 3, heavy 8× (09-28 window). Top: burden Jain over workers; bars are $W=8$, diamonds $W=16$ (measured for `ces` and `ces-k64-home` only); dashed: goal G3. Bottom: bystander p99 schedule-to-poll delay (µs, log scale) on the combiner worker (filled) and the maximum over the other workers (open). ▲: the combiner worker's bystander was never polled in the 2~s window (censored); ×: no bystander was left on the combiner worker (evicted by balancing). Medians of 3 repeats; error bars span [min, max].] <fig:burden>

Yes, if the lock moves ownership off the combiner's worker, as `ces-k64-home` and `fc-remote` do (@fig:burden).

_A chain bound alone does nothing at b0._ `ces-k64` keeps burden at 0.125: the ex-combiner drains its own queue, the next acquirer is again one of its own clients, and its bystander p99 is 171 µs, one 64-hand-off chain. _The bound plus a home break fixes it._ `ces-k64-home` has burden Jain 0.993--0.999 in every cell (8 and 16 workers, sustained and bursty, b0 and b31); its combiner-worker bystander p99 equals the others' (2.6--2.9 µs sustained; 44.7 and 14.9 µs bursty at 8 and 16 workers), and no task starves, at 0.98--0.99× CES throughput sustained and 0.95--0.99× bursty. The cycle budget (`ces-t64000-home`) costs 5--8 %. _For FC, `remote` beats `home`._ `fc-remote` removes the b0 convergence of `fc` (burden 0.981#rng[0.967][0.996] sustained, 1.000 bursty) at the balanced throughput level. `fc-home` reaches only 0.744 at b0 sustained: the ex-combiner's own clients are woken locally and win the next `try_lock`.

The 09-30 window repeats the finding at b31 for the locks of this paper. Burden Jain is #(BurdenSusLo)--#(BurdenSusHi) for every sustained W8 cell of `co-fifo`, `ces-k64-home`, `fc-remote`, `fcpq-h16-home-c16` and `co-pq` (foreign-CS definition for `ces` and `co-*`, combining-cycles definition for FC), and no client or bystander starved in any of the #(Co2Runs) runs (@tab:co2all). In the 09-28 bursty cells our pre-registered hypothesis, a combiner-worker bystander p99 more than one pass above the others', was refuted for all combining variants. The result holds for this executor and workload: without balancing the effect is starvation, not delay, and Q1 does not show what happens with I/O-driven bystanders.

== Q2: Can a fair lock do with ordinary wakers? <sec:q2>

#fig("figures/fig-co2.svg", wide: true)[Per-op cost $o$ against service fairness, $W=8$, 09-30 window, one frozen binary. (a, b) Filled: `spin`; hollow: `yield`; dashed: G1. The FIFO locks sit at Jain 0.67, the usage-ordered ones near 1. (c) $o$ at $W=8$ and $W=16$, sustained; diamonds: `yield`. `tokio-mutex` is the tokio runtime. Medians of 3 repeats; error bars span [min, max].] <fig:co2>

#tbl("tables/tab-co2.typ", wide: true, colsep: 3pt, size: 7.5pt)[Ordinary-waker `co-pq` against the references, $W=8$, b31, heavy 8×, 09-30 window: Mops/s, per-op cost $o$ (cycles), service Jain $J$, and L:H served operations (sustained spin). Upper block: usage-ordered locks (L:H 5.2--5.4 sustained; bursty `fcpq-h16-home-c16` serves L:H 1.2, the others 5.1--5.4). Lower block: FIFO locks (L:H 1.0--1.1). Compare ops/s only inside a block and a load. `co-pq` is the default clamp 256. Medians of 3; spreads and the other metrics in @tab:co2all.] <tab:co2>

Yes for fairness. `co-pq` selects its successor from the usage queue and wakes it with the plain `Waker`; its unlock steps the releaser aside once, and nothing carries a placement hint. It reaches service Jain #(CoPqJainMin) or better with L:H #(CoPqLHLo)--#(CoPqLHHi) in every one of its #(CoPqCells) cells, spin and yield, at $W=8$ and $W=16$, sustained and bursty, with no starved client or bystander (@tab:co2, @fig:co2). Sustained spin gives Jain #(JSusSpinCoPq), against #(JSusSpinCes) for `ces-k64-home` and #(JSusSpinDisp) for `dispatch` at the same load. In bursty cells `co-pq` is fair (#(JBurSpinCoPq)) where the combining lock is not (#(JBurSpinFcpq) for `fcpq-h16-home-c16`): with at most 16 waiters and 7.8 ops per pass, every pass serves everyone pending, so the pass limit never selects.

But it is not cheap. Sustained spin at L:H #(LHSusSpinCoPq): `co-pq` runs at #(ThrSusSpinCoPq) Mops/s with $o=$ #(OSusSpinCoPq) cycles per operation. The combining lock at nearly the same mix (L:H #(LHSusSpinFcpq)) runs at #(ThrSusSpinFcpq) Mops/s and $o=$ #(OSusSpinFcpq): `co-pq` has #(PqOverFcpqThrSusSpin)× its ops/s and #(PqOverFcpqOSusSpin)× its $o$. `dispatch-pq-home-c256` (L:H #(LHSusSpinDpq)) runs at #(ThrSusSpinDpq) with $o=$ #(OSusSpinDpq): `co-pq` is #(PqOverDpqThrSusSpin)× its ops/s and #(PqOverDpqOSusSpin)× its $o$. That contradicts what we expected of the ordinary wake plus async step-aside, which was to land near the executor-aware locks; it is _worse_ than the closure-style hand-off with a home wake. Yield does not help (#(ThrSusYieldCoPq) Mops/s, $o=$ #(OSusYieldCoPq)) and neither does $W=16$ (#(ThrW16SusSpinCoPq), #(OW16SusSpinCoPq)). In the counters of one run every acquisition is a hand-off with one step-aside and every wake is a default placement. We have not broken $o$ down. Our explanation [inference]: the ordinary wake puts the grantee at the back of the releaser's local queue behind the bystanders and other clients, and the releaser's own step-aside adds a second queue pass #todo[breakdown of $o$ for the ordinary-waker hand-off into queue, wake, run-queue wait and poll].

== Q3: What do executor-aware tricks buy on top? <sec:q3>

We compare each trick with its ordinary-waker counterpart at the same mix (@tab:co2, @fig:co2).

#par(first-line-indent: 0pt)[*Inline hand-off (FIFO mix, L:H 1.0).* Sustained spin: `dispatch` runs at #(ThrSusSpinDisp) Mops/s with $o=$ #(OSusSpinDisp); `co-fifo` at #(ThrSusSpinCoFifo) with $o=$ #(OSusSpinCoFifo), #(CoFifoOverDispThrSusSpin)× the ops/s and #(DispOverCoFifoOSusSpin)× lower $o$. `ces-k64-home` (#(ThrSusSpinCes) Mops/s, $o=$ #(OSusSpinCes)) and `fc-remote` (#(ThrSusSpinFc), #(OSusSpinFc)) are the references: `co-fifo` has #(CoFifoOverCesThrSusSpin)× and #(CoFifoOverFcThrSusSpin)× their ops/s. So an inline hand-off with a step-aside runs within 2 % of CES's delegation and within 5 % of combining, and the same holds under yield (#(ThrSusYieldCoFifo) against #(ThrSusYieldDisp) Mops/s for `dispatch`).]

#par(first-line-indent: 0pt)[*Combining (fair mix, L:H $approx$ 5.3).* `fcpq-h16-home-c16` has $o=$ #(OSusSpinFcpq) at Jain #(JSusSpinFcpq), against #(OSusSpinFc) for `fc-remote` at the FIFO mix. Against the ordinary-waker fair locks it is #(PqOverFcpqOSusSpin)× (`co-pq`) and #(DpqOverFcpqOSusSpin)× (`dispatch-pq-home-c256`, $o=$ #(OSusSpinDpq)) cheaper per operation. This is the largest gap we found. Bursty, FC-PQ is unfair (Jain #(JBurSpinFcpq)), so it is not an alternative there.]

#par(first-line-indent: 0pt)[*The async release (when the releaser steps aside).* With a synchronous release, `co-fifo` (`-snone`, the path of `drop(g)`) falls to #(SyncThrSusSpin) Mops/s with $o=$ #(SyncOSusSpin) under spin, against #(AsyncThrSusSpin) and #(AsyncOSusSpin) with the async step-aside: #(SyncOverAsyncThrSusSpin)× sustained and #(SyncOverAsyncThrBurSpin)× bursty ($o$ #(SyncOBurSpin) against #(AsyncOBurSpin), about one 32 000-cycle parallel spin). Under yield the loss is #(SyncOverAsyncThrSusYield)× and #(SyncOverAsyncThrBurYield)×. Under spin the grantee waits behind the releaser's spin unless the releaser steps aside first; that is what `unlock().await` buys. These pairs are from the earlier 09-30 session (`co1-*`, co-fifo unchanged since, both modes and both releases in one window), so we quote ratios only.]

#par(first-line-indent: 0pt)[*Spin against yield.* Sustained, the harness mode moves no lock that steps aside by more than 3 %: yield over spin is #(YieldOverSpinSusCoPq)× for `co-pq`, #(YieldOverSpinSusCoFifo)× for `co-fifo`, #(YieldOverSpinSusCes)× for `ces-k64-home`, #(YieldOverSpinSusFcpq)× for `fcpq-h16-home-c16` and #(YieldOverSpinSusDisp)× for `dispatch` (only `tokio-mutex`, #(YieldOverSpinSusTok)×, moves; §@sec:q5). Bursty, the locks that hand off through an ordinary wake gain from yielding (`co-pq` #(YieldOverSpinBurCoPq)×, `dispatch-pq-home-c256` #(YieldOverSpinBurDpq)×, `dispatch` #(YieldOverSpinBurDisp)×) and the inline locks do not (`co-fifo` #(YieldOverSpinBurCoFifo)×, `ces-k64-home` #(YieldOverSpinBurCes)×), consistent with the ordinary-waker grantee waiting behind the releaser's parallel spin [inference]. Even when the client yields, `dispatch` stays at #(DispOverCesThrBurYield)× `ces-k64-home` bursty and costs #(OSusYieldDisp) cycles per operation sustained. The review's harness worry (the releaser's own spin manufactures the grant-to-run delay) therefore explains the synchronous-release loss above and part of the bursty gap, but not the sustained ordinary-waker cost of Q2.]

#par(first-line-indent: 0pt)[*Bursty.* With 16 clients `co-pq` runs at #(ThrBurSpinCoPq) Mops/s (spin) and #(ThrBurYieldCoPq) (yield) against #(ThrBurSpinCes) / #(ThrBurYieldCes) for `ces-k64-home` (#(PqOverCesThrBurSpin)× / #(PqOverCesThrBurYield)×; L:H 5.1 against 1.1, so not like for like) and #(ThrBurSpinFcpq) / #(ThrBurYieldFcpq) for the unfair `fcpq-h16-home-c16`. Bursty $o$ includes lock-idle time and is not a per-op lock cost.]

#par(first-line-indent: 0pt)[*Summary.* Fairness is a policy and needs only ordinary wakers (Q2). Speed is a mechanism: an inline hand-off with a step-aside, or combining, cuts $o$ by 4--5× at the same mix (Q3). We measured no usage-ordered inline hand-off with the current code #todo[usage-ordered inline hand-off (co-pq with an inline grantee wake, chain bound 64, `home` break) on the current code; an earlier inline co-pq (`co1-*`, other session, superseded implementation) is not comparable].]

== Q4: Does the clamp bind, and what does it cost? <sec:q4>

#fig("figures/fig-service.svg", wide: true)[Service fairness of FC-PQ, $W=8$, sustained, b31. (a) Pass limit $H$ (09-28 window): bars are measured service Jain, ticks the two-class model; dashed: goal G1. (b--f) Starvation-clamp sweep of `fcpq-h16-home`, newcomer init _mean_ (09-29 window): (b) service Jain with the model (ticks) and the confirmation cells (hollow; $W=8$ b0 and $W=16$ b31 were run at clamps 8 and 16 only); (c) light:heavy ops against the ratios the model needs for Jain 0.95 and 1; (d) throughput; (e) heavy-client run latency; (f) worst queue wait, with $c+1$ passes marked. Dashed green: same-window `fc-remote`. Medians of 3 repeats; error bars span [min, max].] <fig:service>

The binding rule of §@sec:policy predicts the units. At $H=16$ the 8-pass clamp binds: L:H stays at 3.64 and Jain at 0.940, below the #LHNinetyFive that Jain 0.95 needs; clamp 16 lifts Jain to 0.997 with a 17-pass worst wait, and no clamp gives the same fairness with a worst wait of 106 passes (@tab:clamp, @fig:service). Counted in hand-offs, the same 16 is FIFO-degenerate: `dispatch-pq-home` at clamp 16 has Jain 0.672 (every request is promoted), and it needs 256 to reach Jain 1.000 (@tab:clamp). `co-pq` at clamp 256 and with no clamp behave identically in the 09-30 window: Jain #(JSusSpinCoPq) and #(JSusSpinCoPqC0), #(ThrSusSpinCoPq) and #(ThrSusSpinCoPqC0) Mops/s. What the clamp does is bound the worst wait: #(WaitSusSpinCoPq) hand-offs at clamp 256 against #(WaitSusSpinCoPqC0) with no clamp (spin; #(WaitSusYieldCoPqC0) under yield). The clamp costs nothing here and bounds the wait; G2 costs no fairness once it is counted in the lock's own tick.

#tbl("tables/tab-clamp.typ", colsep: 4pt)[The clamp in its own unit, $W=8$ sustained b31. `fcpq-h16-home`: passes, 09-29; `dispatch-pq`: hand-offs, 09-29 (wait statistics not recorded); `co-pq`: hand-offs, 09-30 (spin). Max. wait is in the clamp's unit. L:H and Jain are medians of 3.] <tab:clamp>

== Q5: Where does `tokio::sync::Mutex` stand? <sec:q5>

We ran the same workload on tokio's multi-thread runtime (`tokio-bench`: same key stream, costs, histogram and pinning) in the 09-30 window, beside the coro locks and in both harness modes (@tab:co2). *These numbers are for FIFO `tokio::sync::Mutex` only; no usage-ordered lock ran on tokio.* Sustained, it runs at #(ThrSusSpinTok) Mops/s under spin ($o=$ #(OSusSpinTok)) and at #(ThrSusYieldTok) under yield ($o=$ #(OSusYieldTok), #(TokYieldOverSpinSus)× its spin rate). Bursty the factor is #(TokYieldOverSpinBur)× (#(ThrBurSpinTok) to #(ThrBurYieldTok) Mops/s). Under yield it equals the locks that hand off inline (`co-fifo` #(ThrSusYieldCoFifo) and `ces-k64-home` #(ThrSusYieldCes) sustained, #(ThrBurYieldCoFifo) and #(ThrBurYieldCes) bursty) and is #(TokOverDispThrSusYield)× / #(TokOverDispThrBurYield)× our `dispatch`. So the delay we earlier attributed to the lock is the interaction of tokio's LIFO slot (an inline hand-off) with a client that spins after unlocking [inference from the spin/yield contrast; we did not toggle the slot in this window]. Service Jain is FIFO (#(JSusSpinTok)), and no client starved. Three consequences. First, our `dispatch` is not tokio's mutex (it has no LIFO slot), so the two must not be read as one baseline. Second, in tokio the ordinary wake is already an inline hand-off, so the cost of `co-pq` may be a property of our executor rather than of ordinary wakers #todo[`co-pq` on tokio, a second executor]. Third, the advantage of delegation over `tokio::sync::Mutex` that an earlier draft reported (about 1.6× sustained, 4--5× bursty, 09-28) is withdrawn: under yield it is gone.

// ---------------------------------------------------------------------------
= Discussion and Limitations <sec:discuss>

#par(first-line-indent: 0pt)[*Scope of the results.* The critical section is a TSC-timed spin, so locality, the usual argument for delegation, is excluded by construction (§@sec:motiv). One machine, one workload: a `BTreeMap` insert plus a spin, closed-loop, with no application benchmark. Three repeats give min/max spreads, not confidence intervals. Every CES number is for our implementation of its description~@ces. The parallel work has two modes, `spin` and `yield`; a `sleep` mode that releases the CPU needs an executor timer, which ours lacks #todo[`sleep` parallel-work mode; needs an executor timer]. There is one clock window per comparison, on CPUs capped at 3.0 GHz; cross-window ratios are uncertain by about 3 %, and we make none. The CS variant with real memory work (locality) was not run #todo[CS variant with real memory work, to show the direction of the locality term].]

#par(first-line-indent: 0pt)[*Intermittent clients.* Usage is cumulative and never decays. A client that pauses keeps a low counter and is served ahead of everyone on return. In a side experiment of the review (2 light and 2 heavy clients active one 50 ms slot in five, 1.5 s, two repeats, not in this paper's data) an intermittent heavy client received 1.00× the lock time of an always-present heavy client under FC-PQ, and both classes 1.0× under `dispatch-pq`, while FIFO gave 0.19×, yet run-level Jain was 0.98--1.00. Service Jain over the run cannot see this, and G1 is therefore claimed only for clients that contend continuously. A windowed or decayed counter is the obvious fix #todo[decayed or windowed usage, and an intermittent-client scenario in the harness]. *Bystanders and balancing.* Steal-when-idle never fires with always-runnable bystanders; our balancing steal substitutes for it, and burden and trapped-client results depend on that choice (§@sec:forms).]

#par(first-line-indent: 0pt)[*Clamp units.* A starvation clamp means something only in its lock's tick: clamps that are safe in FC-PQ passes make `dispatch-pq` FIFO when counted in hand-offs (§@sec:policy), and our hand-off clamps of 256 and off were chosen after an exploratory single-repeat pass showed this, not pre-registered. By the binding rule, Jain 0.95 / 0.99 would need a hand-off clamp $c gt.tilde 149 \/ 184$ [inference, untested] #todo[`dispatch-pq` clamps 150--220 hand-offs, to trace the predicted Jain frontier]. *Uncontended fast path.* In every measured cell every `dispatch-pq` and `co-pq` acquisition was a hand-off (fast share 0); the one-CAS fast path is exercised only by unit tests, so low-contention cost is unmeasured #todo[`dispatch-pq` and `co-pq` at low contention, where the fast path is taken].]

*What we withdrew.* An adversarial review found that parts of an earlier version were artefacts. Each item states why in one sentence.
- _"Delegation is the fast realisation"; a fair hand-off runs at about half of FC-PQ's utilisation, with a 3.9--5.6 k-cycle grant-to-run delay._ The delay was measured under the spin harness and with default placement only; the hand-off cost is a property of the wake (ordinary against inline), and an inline FIFO hand-off matches combining (Q3).
- _Utilisation, the utilisation identity and every fair-against-FIFO ops/s ratio._ The CS is a timed spin, so utilisation counts spin as useful work, and usage order changes the mix; we report $o$ at a stated mix.
- _tokio's LIFO slot as a form of subversion, and the delegation advantage over `tokio::sync::Mutex` (about 1.6× sustained, 4--5× bursty)._ Under yield tokio's mutex matches the inline locks (Q5), and our `dispatch` was never tokio's mutex.
- _G1 as equal lock time for every client, and "service Jain 1.000" as the figure of merit._ Cumulative usage rewards absence and run-level Jain cannot see it, so the claim is rescoped to continuously contending clients.
- _The actor control (a server task is not enough)._ It was measured only under the spin harness with default wake placement and not rerun under yield, so it is dropped rather than repeated.
- _Burden Jain as one comparable number._ FC-family and `ces` / `co-*` locks use different definitions (combining against foreign-CS cycles), which §@sec:motiv now states.

#par(first-line-indent: 0pt)[*Future work.* A usage-ordered inline hand-off on the current code; `co-pq` on tokio and a second executor; a breakdown of the ordinary-wake cost; decayed usage; I/O-driven bystanders; real applications.]

// ---------------------------------------------------------------------------
= Related Work <sec:related>

#par(first-line-indent: 0pt)[*Scheduler-cooperative locks.* SCL~@scl names _scheduler subversion_ for OS threads and fixes it with usage accounting and lock-slice penalties. Its user-space u-SCL is the non-delegating usage-ordered lock: each thread runs its own critical section. `co-pq` and `dispatch-pq` are, in effect, its async analogue (usage order and a starvation clamp, but no lock slices, hence the unbounded accounting horizon of §@sec:discuss). SCL's §7 already suggests usage-fair scheduling of delegated requests, so combining the two is not our claim. CFL~@cfl schedules lock occupation by cgroup share and priority, NUMA-aware. We share their goal of fair lock _time_; the executor adds the _where_.]

#par(first-line-indent: 0pt)[*Delegation locks.* FC~@fc is starvation-free and linearizable and lists the number of consecutive combining rounds as a knob; TCLocks~@tclocks bound combining batches at 1 024 waiters for "long-term fairness", with no fairness metric. Neither measures a per-thread combiner share; we measure it and move it. The FC dispatcher poster~@fcdispatch rotates the dispatcher role, which mitigates burden, but does not measure the cost.]

#par(first-line-indent: 0pt)[*CES.* CES~@ces states that it is the first delegation-style lock for cooperatively scheduled tasks; the claim is theirs. It evaluates throughput only, at uniform CS cost. Its motivating observation, that a dispatched hand-off puts a scheduler round trip on the critical path, is what §@sec:q3 measures for a FIFO hand-off; we add chain bounds with break placement, the burden metric, usage-ordered service and an async unlock as the step-aside point.]

#par(first-line-indent: 0pt)[*Async runtimes and locks.* The tokio mutex~@tokio is a FIFO semaphore whose hand-off wake goes through the LIFO slot, a bounded inline-after-poll hand-off on the unlocker's worker [inference from its source, consistent with §@sec:q5]; its fairness scope is only the waiters~@tokio6049. `async-lock`~@asynclock relies on a 0.5 ms starvation hand-off, which never fires when the notified waiter is never polled.]

#par(first-line-indent: 0pt)[*Preemptive request scheduling.* Shinjuku~@shinjuku preempts every 5--15 µs to handle heterogeneous request costs and places contended locks out of scope; Perséphone's DARC~@persephone reserves cores per request type. Both handle cost heterogeneity in the request scheduler; we handle it inside the lock, where a cooperative executor has no preemption to fall back on.]

// ---------------------------------------------------------------------------
= Conclusion <sec:concl>

In this paper, we have shown that a lock in a coroutine runtime is a hidden scheduler that decides who runs next and, with an explicit async unlock, when the releaser steps aside. At its defaults it subverts the executor: FIFO service in proportion to cost, monopoly, worker seizure and, for delegation, combiner burden, which lock-side placement repairs (burden Jain #(BurdenSusLo)--#(BurdenSusHi)). Service fairness is a lock policy that needs only ordinary wakers: `co-pq` reaches service Jain #(CoPqJainMin) or better in #(CoPqCells) cells. What the executor-aware mechanisms buy is speed: at one mix, an inline hand-off or combining costs #(OSusSpinFc)--#(OSusSpinCoFifo) cycles per operation (`fc-remote` to `co-fifo`, FIFO mix; #(OSusSpinFcpq) for FC-PQ at the fair mix), where an ordinary-waker hand-off costs #(OSusSpinDisp) (FIFO) to #(OSusSpinCoPq) (fair), and that cost is a property of the runtime's wake placement. Our hope is that runtimes will let a lock choose where its wakes run, so that fairness and speed no longer need to be traded.

#[
#set text(size: 9pt)
#bibliography("refs.bib", style: "ieee", title: "References")
]

// ---------------------------------------------------------------------------
#counter(heading).update(0)
#set heading(numbering: "A.1")
= Supplementary Tables <sec:appendix>

@tab:co2all gives every cell of the `co2` matrix with the [min, max] spreads of Mops/s and $o$; @tab:loc gives the size of each part. The tables are generated by `make_tables.py` from the result files.

#tbl(size: 6.5pt, "tables/tab-co2-detail.typ", wide: true, colsep: 3pt, placement: bottom)[All cells of the ordinary-waker matrix (09-30 window; median of 3, [min, max] for Mops/s and $o$). Burden $J$: foreign-CS Jain for `ces`, `co-*`; combining-cycles Jain for `fc`, `fcpq`; `--` where undefined. Byst. p99: worst worker's bystander schedule-to-poll p99. Latencies are run-latency p99 (lock() to unlock().await for `co-*`, `ces`). Starved: clients / bystanders with no progress. `co-pq` is clamp 256, `co-pq-c0` no clamp.] <tab:co2all>

#tbl(size: 7pt, "tables/tab-loc.typ", placement: bottom)[Lines of Rust: non-blank, non-comment lines, with lines from the first `#[cfg(test)]` on counted as tests. Counted by `make_tables.py`.] <tab:loc>
