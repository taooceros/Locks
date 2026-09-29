#import "lib.typ": rng, fit
#import "tables/numbers.typ": *
#import "figures/contract.typ": contract

// A number or result the text wants but no FINDINGS.md entry or JSON provides.
#let todo(body) = text(fill: red, weight: "bold")[\[TODO: #body\]]

#let title = [Locks Are Hidden Schedulers:\ Avoiding Executor Subversion in Cooperative Runtimes]

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
    Coroutine runtimes multiplex many tasks onto a few worker threads and schedule them cooperatively. We show that a lock in such a runtime is a _hidden scheduler_: it decides which task runs next and on which worker, and the executor sees neither decision. At their defaults these decisions _subvert_ the executor in five measured forms: FIFO locks, `tokio::sync::Mutex` included, give lock time in proportion to critical-section cost (service Jain 0.65--0.66 for a 1:8 mix); `async-lock` hands the lock to one client; blocking mutexes seize the workers; tokio's LIFO slot parks each new owner behind the unlocker; and delegation locks concentrate combining on one worker (burden Jain $1\/W$). Service fairness needs no delegation: one usage-ordered queue reaches service Jain #(DpqFairJainMin) behind a plain hand-off mutex and 0.997 inside a flat-combining lock (FC-PQ). On our executor, though, each fair hand-off is a wake-schedule-poll round trip, so the hand-off lock reaches only #(DpqUtilFcLo)--#(DpqUtilFcHi)× FC-PQ's utilisation. Delegation's own subversion, combiner burden, yields to lock-side wake placement (burden Jain 0.98--1.00 at 0.95--1.01× CES throughput), and the burden-fair locks run #(SusOnLo)--#(SusOnHi)× `tokio::sync::Mutex` under sustained load. Fairness costs utilisation through a per-op lock cost of about 1 150 cycles (0.84 to 0.68).
  ]
]

// ---------------------------------------------------------------------------
= Introduction <sec:intro>

Coroutines are back in wide use: many modern languages support cooperative multitasking~@ces. Runtimes such as tokio~@tokio multiplex many tasks onto a few worker threads. A task runs until it returns `Pending`, and workers balance load by stealing. Fairness guarantees are weak and cover only runnable tasks, or only a lock's waiters~@tokio6049. A lock in such a runtime is usually an _async mutex_: a waiter suspends, and the unlocker wakes it through the scheduler. CES~@ces observed that this puts a scheduler round trip on the critical path and resumes the next waiter inline instead, which makes it a delegation lock, like flat combining (FC)~@fc and TCLocks~@tclocks.

Patel et al.~@scl showed that ordinary locks _subvert_ the OS scheduler: lock hold time, not the scheduler's share, decides who gets CPU time. A cooperative executor is exposed to more than that, because it cannot preempt, and because a lock also decides _where_ work runs. We call this _executor subversion_: lock behaviour, not the executor's policy, decides which task runs next, on which worker, and for how long. Every lock is thus a _hidden scheduler_. Its grant order picks the next critical section; its unlock path picks where that section runs, whether on the woken task's worker, on a blocked thread or inside another task's combining pass. The executor sees neither decision.

As a simple example, take $W=8$ workers, 64 clients, half with 8× the critical-section (CS) cost of the other half, and one always-runnable bystander per worker (@fig:motiv). Under strict FIFO every client gets equal operations, hence lock time in proportion to cost: the predicted service Jain index is 0.653 (from the measured 1 306:8 324 cycles per op), and `dispatch`, CES, FC and `tokio::sync::Mutex` all reproduce it to within 0.01 (@fig:motiv a). `async-lock` hands the lock to a single client in #(AsyncMonoRuns) of #(AsyncRuns) runs, and `std` and `parking_lot` mutexes block the first $W$ clients' workers for the whole run (@fig:motiv b). `tokio::sync::Mutex` wakes each new owner into the unlocker's LIFO slot, behind the unlocker's own work; disabling the slot lifts its bursty throughput 3.4--3.7× (@fig:motiv c). A delegation lock adds a form of its own: with CES the unlocking worker combines for the entire 2 s run (burden Jain $1\/W$), and the bystander on that worker is never polled (@fig:motiv d, e).

#par(first-line-indent: 0pt)[*The crux.* Can a lock in a cooperative executor give clients equal lock time, keep every client and every worker served, and stay fast, and what is the least it must ask of the executor?]

In this paper, we separate the policy that restores service fairness from the lock that carries it. The policy is lock-agnostic: a usage-ordered queue (lowest cumulative charged CS cycles first, with a starvation clamp) that we share, unchanged, between a plain hand-off mutex (`dispatch-pq`) and a flat-combining lock (FC-PQ). The executor contract is equally small: a placement argument on wakes (inline, remote, home), which is also all that delegation's own subversion, combiner burden, needs.

We find that usage ordering needs no delegation: behind a hand-off mutex it reaches service Jain #(DpqFairJainMin). What differs is the price of each fair decision. A hand-off that wakes the grantee through the scheduler spends #(DpqGtsLo)--#(DpqGtsHi) cycles between grant and CS start, #(DpqOOverFcpqLo)--#(DpqOOverFcpqHi)× FC-PQ's whole per-op lock cost, so fair `dispatch-pq` runs at #(DpqUtilFcLo)--#(DpqUtilFcHi)× FC-PQ's utilisation; a combiner makes the same decision inside its pass and runs the chosen closure itself. Delegation is therefore the fast realisation we measured, not a necessary one: an inline hand-off that resumes the chosen waiter on the releasing worker is untested. Delegation also brings combiner burden, which lock-side placement fixes (burden Jain 0.98--1.00). Burden-fair delegation runs #(SusOnLo)--#(SusOnHi)× `tokio::sync::Mutex` under sustained load, and a per-lock server task (actor) gets throughput or burden fairness, not both.

Our contributions are:
+ Measurements of five forms of executor subversion across hand-off, blocking and delegation locks in two runtimes; FIFO service fairness follows from per-class costs to three digits (§@sec:motiv).
+ A lock-agnostic usage-ordered policy, with a rule for when its starvation clamp binds in pass or hand-off units (predicted Jain 0.9395, measured 0.940), and a minimal executor contract (§@sec:design).
+ Two realisations of the policy and an account of their per-op cost: a hand-off mutex (service Jain #(DpqFairJainMin), with grant-to-run #(DpqGtsShareLo)--#(DpqGtsShareHi) % of its per-op cost) and a combining lock (0.997), plus wake-placement policies for combiner burden (§@sec:design, §@sec:eval).
+ An evaluation, including a utilisation identity that separates per-op lock cost from the served mix (§@sec:eval).

#par(first-line-indent: (amount: 1em, all: true))[The rest of this paper is organized as follows. §@sec:motiv demonstrates the five forms and §@sec:goals turns them into goals. §@sec:design and §@sec:impl present the contract, the policy and its two realisations, and §@sec:eval evaluates them. §@sec:discuss–§@sec:concl discuss limitations, related work and conclusions.]

// ---------------------------------------------------------------------------
= Background and Motivation <sec:motiv>

We first describe the executor, locks and workload, then demonstrate the five forms of subversion (@fig:motiv).

== Cooperative executors
A tokio-style executor~@tokio runs $W$ worker threads. Each has a local run queue, and there is one shared injector queue. An idle worker steals half of a random peer's queue. tokio adds two policies that matter here: a _LIFO slot_ (a task woken from a worker is polled right after the current poll, cannot be stolen, and is capped at 3 in a row) and a _cooperative budget_ of 128 units per poll, consumed by `tokio::sync::Mutex` acquires.

Our executor (§@sec:impl) is built from `async-task`~@asynctask and `crossbeam-deque`~@crossbeam. It has per-worker FIFO queues, an injector and a single run-next slot. Every 31 polls a worker drains up to $l e n \/ W+1$ injector tasks (tokio's share rule). In our workload every worker hosts an always-runnable bystander, so steal-when-idle never fires. We therefore add a rate-limited _balancing steal_: every $b$ polls, a busy worker takes half of a random peer's queue. Runs use $b=31$ ("b31") or no balancing ("b0").

== Locks
We use the taxonomy of CES~@ces. Under _dispatch_ (`dispatch`; tokio, Kotlin, Boost), unlock enqueues the next waiter and the owner continues. Under _inline_, unlock resumes the waiter recursively. Under _CES_ (`ces`), unlock reschedules the owner remotely and resumes the head waiter inline. In _FC_ (`fc`)~@fc, clients publish closures, and whoever wins a `try_lock` runs a pass of up to $H$ of them. `dispatch-pq` and FC-PQ (`fcpq`) are `dispatch` and FC with a usage-ordered queue (§@sec:policy).

== Workload and metrics
The protected structure is a `BTreeMap<u64,u64>` with 65 536 keys. A critical section (CS) is one insert plus a TSC-timed spin: 1 000 cycles for light clients and 8× for heavy clients, half the clients each. Under _sustained_ load 64 clients do 4 000 cycles of parallel work between operations; under _bursty_ load 16 clients do 32 000 cycles. $W$ bystander tasks spin 1 000 cycles and then yield. We use two Jain indices~@jain, $J=(sum x_i)^2 \/ (n sum x_i^2)$: _service Jain_ over per-client CS cycles and _burden Jain_ over per-worker combining cycles. The _combiner-worker bystander p99_ is the p99 schedule-to-poll delay on the worker with the most combining cycles, against the maximum p99 of the other workers. A bystander first polled after the window is _censored_ ("cens." in the tables): it never ran.

#fig("figures/fig-motivation.svg", wide: true)[Five forms of executor subversion, heavy 8×. (a) FIFO service unfairness: per-client CS cycles over the mean, $W=8$ sustained b31; dashed: FIFO theory at `dispatch`'s measured costs. (b) Monopoly and worker seizure (tokio runtime): share of clients (bars) and bystanders (◇) that completed nothing in 2~s. (c) LIFO hand-off delay: `tokio::sync::Mutex` with the LIFO slot on and off (throwaway `tokio_unstable` build). (d) Combiner burden: share of combining cycles per worker, sorted, $W=8$ sustained b0. (e) Bystander p99 schedule-to-poll delay on the combiner worker (filled) and the maximum elsewhere (open), same runs. All runs are from the 09-28 window except `fcpq-h16-home-c16` (hollow, 09-29). Medians of 3 repeats; error bars span [min, max].] <fig:motiv>

== Five forms of subversion, measured <sec:forms>
@fig:motiv summarizes the five forms; the paragraphs below separate them. The coro matrix covers $W in {4,8,16}$, heavy $in {1,8}$ and $b in {0,31}$ (432 runs); the tokio runs cover $W in {8,16}$, sustained and bursty. The first two forms decide _who_ runs next, the other three _where_.

#par(first-line-indent: 0pt)[*FIFO service is proportional to cost (every FIFO lock).* Under `dispatch` and `tokio::sync::Mutex` every light client receives 0.28× and every heavy client 1.71--1.72× the mean lock time, the FIFO prediction $2 C_L \/ (C_L+C_H)$ and $2 C_H \/ (C_L+C_H)$ (@fig:motiv a). The two-class model $J(x)=(1+x)^2 \/ (2(1+x^2))$, with $x$ the light:heavy per-client service ratio, predicts service Jain from each lock's measured costs to three digits for `dispatch`, CES, FC and `tokio::sync::Mutex` (@tab:fifo). Equal service would need 6.0--6.5 light operations per heavy one.]

#tbl("tables/tab-fifo.typ", colsep: 3pt)[FIFO service fairness, $W=8$, sustained, b31 (09-28 window). $"CS"_(L,H)$ are critical-section cycles per op (TSC). The model uses $x="L:H" dot "CS"_L \/ "CS"_H$. The last column is $"CS"_H$/$"CS"_L$.] <tab:fifo>

#par(first-line-indent: 0pt)[*Monopoly (`async-lock`).* `async-lock`~@asynclock collapses to a single client in #(AsyncMonoRuns) of #(AsyncRuns) tokio runs: 63 of 64 or 15 of 16 clients starve (@fig:motiv b). With the LIFO slot on it does so in 20 of 24 runs, with it off in 0 of 12.]

#par(first-line-indent: 0pt)[*Worker seizure (blocking mutexes).* `std` and `parking_lot` mutexes block the worker thread, so the first $W$ clients polled keep the $W$ workers for the whole run: starved clients equal clients $- W$, and all $W$ bystanders starve (@fig:motiv b).]

#par(first-line-indent: 0pt)[*Hand-off delay (tokio's LIFO slot).* A `tokio::sync::Mutex` hand-off wake lands in the unlocker's LIFO slot, and the new owner waits there behind the unlocker's parallel work while the lock is held for it. Disabling the slot lifts `tokio::sync::Mutex` 3.4--3.7× in bursty mode and 1.08× in sustained mode (@fig:motiv c; @tab:lifo).]

#par(first-line-indent: 0pt)[*Combiner burden (delegation only).* In every saturated sustained cell CES has burden Jain exactly $1\/W$ (0.125 at 8 workers, 0.062 at 16), with or without balancing (@fig:motiv d): the median chain is 819 200 inline hand-offs, one chain for the whole window. Without balancing, CES traps what sits in the combiner's queue: 5 clients (5--6 across repeats) and the bystander starve for 2 s (@fig:motiv e), and service Jain drops to 0.574#rng[0.557][0.627]. With balancing, the bystander is stolen during warm-up and never returns. Whether a chain ends is an executor property: an earlier executor that drained one injector task per 31 polls let the queue empty at $W <= 8$ (burden Jain 0.96 at 8 workers, 0.06 at 16). FC without a yield is worse: without balancing, served waiters are woken into the combiner's own queue and the client loop never returns `Pending`, so one task re-wins the election forever (service Jain $0.016=1\/64$). A yield after combining ends the starvation, but at b0 all clients still converge on the combiner's worker (burden $1\/W$; 0.235 instead of 0.390 Mops/s with balancing).]

#par(first-line-indent: 0pt)[*Summary.* At their defaults, locks in a cooperative executor hand out lock time in proportion to cost or to luck, and decide where work runs by blocking workers, queueing new owners behind their unlockers, or concentrating combining on one worker; only the last is specific to delegation. The executor cannot repair them: it cannot preempt a blocked or combining worker, and balancing evicts a trapped bystander rather than serving it.]

// ---------------------------------------------------------------------------
= Design Goals <sec:goals>

We turn these requirements into goals, following SCL~@scl; each goal answers a form of §@sec:motiv.
#[
#set enum(numbering: n => strong[G#n])
+ *Service fairness.* Clients receive equal lock _time_, not equal operations: service Jain $>= 0.95$ under 1:8 cost heterogeneity, which needs usage accounting (FIFO: 0.65--0.66).
+ *Bounded wait.* No request waits more than a fixed number of lock decisions, whatever its usage (`async-lock` and FC without a yield fail it).
+ *Worker fairness.* No worker is seized by lock work: burden Jain $>= 0.9$, and the combiner worker's bystander p99 within one histogram bucket of the others' (blocking mutexes and CES fail it).
+ *Low per-op cost.* The fair policy adds little lock time per operation and no idle lock time.
+ *Executor independence.* The lock asks the executor only for wake placement: no priorities, preemption, scheduler callbacks or lucky injector drain rate.
]
G1 and G4 conflict in one measurable way: fairness shifts the served mix toward cheap operations, and each operation carries a fixed lock cost. We quantify this conflict in §@sec:q5 rather than hide it in an ops/s number.

// ---------------------------------------------------------------------------
= Design <sec:design>

#figure(placement: top, scope: "parent", kind: image, contract,
  caption: [The executor contract and the two burden policies. (a) A lock may ask for each wake to be placed _Inline_ (the waking worker's run-next slot), _Remote_ (the shared injector) or _Home_ (the inbox of the worker that last polled the wakee). (b) CES resumes waiters inline on one worker; `ces-k64-home` ends the chain after $K=64$ hand-offs and hands ownership to the next waiter on its home worker, so the ex-combiner drains its own queue. (c) An FC combiner serves at most $H$ closures, wakes the served waiters remotely (`fc-remote`) and yields once after combining, so the tasks queued on its worker run before it competes again.]) <fig:contract>

This section presents the executor contract that G5 permits, a usage-ordered policy that meets G1 and G2 for any lock, its two realisations, and the placement policies that meet G3 for delegation locks (@fig:contract).

== Executor contract
A `Waker` can only enqueue its task, so it carries no placement. The executor therefore exposes a thread-local placement hint, which the schedule callback reads and resets (@fig:contract a); this hint is all a lock asks for (G5). `wake_with(Inline, w)` puts the task in this worker's run-next slot, polled as soon as the current poll returns; the slot has no anti-starvation cap, so that unbounded CES chains remain observable. `wake_with(Remote, w)` pushes the task to the injector and unparks one worker; `wake_with(Home, w)` pushes it to the inbox of the worker that last polled it. `reschedule_self_remote(cx)` marks the _current_ task `Remote`, applied when its poll returns `Pending`. Tokio's LIFO slot is an implicit, capped `Inline` for every wake issued on a worker; the contract makes that choice explicit and per-wake.

== A lock-agnostic usage-ordered policy <sec:policy>
G1 needs usage accounting, which no FIFO lock has. Our policy is a queue, `UsageQueue`, generic over the waiter type and shared unchanged by both fair locks. It keeps waiting requests in a binary min-heap keyed by (cumulative charged cycles, arrival). The lock charges each CS the `rdtscp` cycles around it, and the charge persists in the client's node. Two rules modify the key. A _newcomer_, meaning a client never served before, enters at the lock's running mean cost per request; zero, the minimum and the median are alternatives. The _starvation clamp_ enforces G2. At every _tick_, an entry that has waited more than $c$ ticks has its key lowered to the current heap minimum until it is served; the accounting is untouched. The tick is the lock's unit of decision: a combining pass in FC-PQ, a hand-off in `dispatch-pq`. (Clamping at the tick fixes a no-op in the reference FC-PQ, which clamped after the pop.)

#par(first-line-indent: 0pt)[*When does the clamp bind?* A clamp shorter than the wait that usage ordering would impose overrides the ordering, so G2 can defeat G1. Take FC-PQ with pass limit $H=16$ and $N_h=32$ heavy clients. A heavy client's service interval is $2(1+"L:H")$ passes. Without the clamp, usage ordering settles where charged usage is equal, at an interval of $2 times 6.51=13.0$ passes. A clamp $c$ caps the interval at $(c+1)+0.28$ passes, where 0.28 is the return delay (parallel work plus wake). A clamp therefore binds only if $c+1.28<13.0$, i.e. $c <= 11$.]

Clamp 8 binds: #PromotedCEight of requests are promoted, the heavy share of operations $1\/(1+3.64)$, and the interval $(c+1)+0.28=9.28$ passes gives L:H $=3.64$. Clamps of 12 and more do not bind: clamp 16 promotes at most 0.035 % of requests, all in the wait tail.

The same rule, counted in hand-offs, sets `dispatch-pq`'s clamp. Under usage order a light request waits 36 hand-offs (p50), and a heavy client is served once per round of 32 heavy and $32 times 5.84$ light operations, about 219 hand-offs. A clamp of 8 or 16 hand-offs is shorter than even the light wait, so it promotes every request. Promoted keys collapse to the running minimum, ties go by arrival, and service is FIFO. A hand-off clamp must be about 16× looser than a 16-op-pass clamp; we use 256 (`-c256`) or none (`-c0`).

Service Jain follows from the two-class model with $x="L:H" dot "CS"_L \/ "CS"_H$: it predicts 0.9395 at clamp 8 (measured 0.940) and 0.9970 at clamp 16 (measured 0.997). $J >= 0.95$ needs L:H $>= #LHNinetyFive$ ($x >= #XNinetyFive$), i.e. $c >= 9$; for the unrun clamps 9--11 the model predicts $J approx 0.963$, 0.981 and 0.992. FC-PQ stops at 0.997 because its charge window includes the trampoline, about 193 cycles per operation that the harness's CS timer does not see; `dispatch-pq` charges only the closure and reaches 1.000 [inference].

#par(first-line-indent: 0pt)[*Utilisation.* G1 and G4 meet in one identity. Let $macron(C)$ be the mean CS cycles per operation of the served mix and $o=(T-C S) \/ o p s$ the per-operation cycles not spent in a CS. Then lock utilisation is $C S \/ T = macron(C) \/ (macron(C)+o)$. The identity holds by definition; its content is empirical. For a lock that is always held or in hand-off, $o$ is the lock's per-op cost; for FC-family locks under sustained load (a combiner busy 98.8--98.9 % of the window) it is in-pass administration plus hand-off gap, nearly independent of the clamp (#OCEight cycles at clamp 8, #OCSixteen at 16). Fairness moves the mix toward cheap operations, which lowers $macron(C)$ (#CbarCEight to #CbarCSixteen cycles) and therefore utilisation.]

== Two realisations: hand-off and combining <sec:realise>
#par(first-line-indent: 0pt)[*Hand-off: `dispatch-pq`.* The lock word is `UNLOCKED`, `LOCKED` or `LOCKED+QUEUED`. An uncontended acquire is one CAS and never touches the queue. A contended acquire takes a TTAS spinlock; it takes the lock if it has become free, and otherwise sets `QUEUED` and pushes its waiter. Release is one CAS when `QUEUED` is clear. Otherwise it takes the spinlock, advances the hand-off clock, applies the clamp and pops the minimum. Ownership passes directly: the lock stays held, the grantee's `granted` flag is set (release), and the grantee is woken with the lock's placement. Each owner runs and charges its own CS on its own worker, as in `dispatch`.]

#par(first-line-indent: 0pt)[*Combining: FC-PQ.* Every FC client owns one node, allocated once. A request writes a closure pointer and a trampoline into the node, pushes it onto a Treiber stack and tries the combiner flag. The winner drains the stack into the policy queue (FIFO for `fc`, `UsageQueue` for FC-PQ) and runs at most $H$ closures (default 64), marking each served waiter `COMPLETE` (release/acquire) and waking it with the lock's _wake placement_. Losers return `Pending`; a waiter never spins. At pass end the combiner releases the flag and re-checks the stack (SeqCst on both sides); once its own request is served it wakes the policy's next candidate, which re-runs the election. The pass limit decides whether ordering matters at all: when $H >=$ the number of waiters, every pass admits everyone and the share stays FIFO's.]

#par(first-line-indent: 0pt)[*Why a hand-off pays per decision.* In `dispatch-pq` every fair decision grants the lock to a suspended task. Its CS cannot start until the executor has woken, scheduled and polled it, and the lock is held for it all that time: one scheduler round trip per decision, on the critical path. The combiner makes the same decision inside its pass and runs the chosen closure itself. It waits for nobody to be scheduled; the chosen client is woken only after its CS has run, off the critical path. CES made the same observation for FIFO hand-off~@ces and resumes the next waiter inline instead. An inline hand-off with usage order, a "`ces-pq`", would avoid the round trip without closure delegation; we have not built it. §@sec:qdel measures the difference between the two realisations.]

== Combiner burden: bounds and placement <sec:burden>
#par(first-line-indent: 0pt)[*CES chain bound and break placement.* A CES chain that never ends violates G2 and G3. The fix has two parts: a bound that ends the chain, and a placement that starts the next one elsewhere. A _chain_ is the sequence of inline resumes on one worker since that worker last polled anything else (@fig:contract b). `ces-k`$K$ ends a chain after $K$ hand-offs; `ces-t`$T$ ends it after $T$ cycles. At that point the unlocker hands ownership to the head waiter with a _break placement_ instead of resuming it inline, and continues, so the worker drains its own queue when the poll returns. With the default break placement, the woken owner lands in the ex-combiner's own queue and the next chain restarts there. The `-home` suffix sends it to its home worker's inbox, which moves the combiner role (§@sec:q1).]

#par(first-line-indent: 0pt)[*FC yield and placement.* FC fails in two ways (§@sec:motiv): a combiner poll that never returns `Pending` starves every other client (G2), and wakes onto the combiner's worker pull all clients there (G3). _Yield-after-combine_ (default on) fixes the first: a poll that won the election returns `Pending` once, having woken itself behind the waiters it just served (@fig:contract c). Preemption gives OS-thread FC the same effect. Wake placement fixes the second: `-remote` and `-home` set the placement of every wake the lock issues.]

== The obvious alternative: an actor
An application that wants G3 without a delegation lock would write a server task. `actor` spawns one server task per lock, lazily with the first request. Each server poll drains the request stack, serves up to 64 closures in FIFO order and yields. The server parks when there is nothing to serve. `actor-inline` changes three placements: the server is woken into the publisher's run-next slot, it yields `home`, and it wakes clients `remote`.

// ---------------------------------------------------------------------------
= Implementation <sec:impl>

The locks, executor and harness are written in Rust. @tab:loc in the appendix gives the size of each part.

#par(first-line-indent: 0pt)[*Executor and requests.* Tasks are `async-task` tasks; each worker has a `crossbeam-deque` FIFO and a run-next slot, and there is one shared `Injector`. Idle workers park through a SeqCst-fenced registration protocol that forbids lost wake-ups. Clients never allocate per request: each owns one node or waiter, which the lock keeps alive. All locks share one `rdtscp` counter with the harness.]

#par(first-line-indent: 0pt)[*Checks.* A counting-allocator probe in steady state measured 0~allocations per op for `fc`, `actor` and `dispatch-pq` (default wakes); the executor queues add 0.012--0.016 per op for `fc-remote`, 0.016--0.025 for `actor-inline` and 0.0159 for `dispatch-pq` with home or remote wakes (one `Injector` block per 63 pushes). `dispatch`, `ces` and `fcpq` were not probed #todo[allocation probe for `dispatch`/`ces`/`fcpq` not recorded]. The `actor` tests (mutual exclusion with overlap detection, no lost wake-ups across idle transitions, FIFO order) and the six `dispatch-pq` tests (minimum-usage grant order, the clamp in hand-offs, dropping a queued or granted request, both sides of the release/enqueue race, mutual exclusion on the real executor) pass in release builds and under ThreadSanitizer (0 reports). We have no TSan record for the other locks #todo[TSan runs for `dispatch`/`ces`/`fc`/`fcpq` not recorded]. `coro-bench --sanity` and `tokio-bench --sanity` (map length equals total ops) pass for all locks.]

// ---------------------------------------------------------------------------
= Evaluation <sec:eval>

In this section, we answer six questions, one per subsection.

#par(first-line-indent: 0pt)[*Setup.* 2× Intel Xeon Gold 6438M (32 cores per socket, SMT on), Linux 6.17.7, a 2.20 GHz TSC, `rustc` 1.100.0-nightly `--release`. $W$ workers are pinned to logical CPUs $0..W-1$, one per physical core. Each run has a 200 ms warm-up and a 2 s window; each configuration is repeated 3 times, repeats outermost, one run at a time under a measurement lock. We report medians with [min, max] and do not interpret differences inside the spread. Latencies are histogram bucket lower bounds at 6 % resolution. One unrelated single-threaded process ran during the 09-28 matrices. #todo[commit id of the measured binaries not recorded; binaries are identified by sha256 in FINDINGS.md]]

#par(first-line-indent: 0pt)[*Two measurement windows.* From 2026-09-29 00:07:54 UTC, CPUs 0--15 were capped at 3.0 GHz (09-28: about 3.69 GHz turbo). Spins are TSC-timed, so the cap slows only non-spin work (a light CS costs 1 365--1 376 instead of 1 303 cycles). Every table row and figure panel names its window (09-29 data are hollow where one axis shows both), and ratios are formed within one window unless marked. Binaries re-run on 09-29 were 1.4--3.4 % slower than their 09-28 cells, outside both spreads, which we attribute to the cap [inference]; cross-window ratios are therefore uncertain by about 3 %.]

== Q1: Does placement restore burden fairness? <sec:q1>

#fig("figures/fig-burden.svg", wide: true)[Burden fairness, phase 3, heavy 8× (09-28 window). Top: burden Jain over workers; bars are $W=8$, diamonds $W=16$ (measured for `ces` and `ces-k64-home` only); dashed: goal G3. Bottom: bystander p99 schedule-to-poll delay (µs, log scale) on the combiner worker (filled) and the maximum over the other workers (open). ▲: the combiner worker's bystander was never polled in the 2~s window (censored); ×: no bystander was left on the combiner worker (evicted by balancing). Medians of 3 repeats; error bars span [min, max].] <fig:burden>

Yes, if the lock moves ownership off the combiner's worker, as `ces-k64-home` and `fc-remote` do (@fig:burden).

_A chain bound alone does nothing at b0._ `ces-k64` keeps burden at 0.125: the ex-combiner drains its own queue, the next acquirer is again one of its own clients, and its bystander p99 is 171 µs, one 64-hand-off chain. _The bound plus a home break fixes it._ `ces-k64-home` has burden Jain 0.993--0.999 in every cell (8 and 16 workers, sustained and bursty, b0 and b31); its combiner-worker bystander p99 equals the others' (2.6--2.9 µs sustained; 44.7 and 14.9 µs bursty at 8 and 16 workers), and no task starves, at 0.98--0.99× CES throughput sustained and 0.95--0.99× bursty. Bursty CES chains already end naturally (p50 12 hand-offs); there the bound only trims the tail from 152--191 to 64, and the cycle budget (`ces-t64000-home`) costs 5--8 %.

_For FC, `remote` beats `home`._ `fc-remote` removes the b0 convergence of `fc` (burden 0.981#rng[0.967][0.996] sustained, 1.000 bursty) at the balanced throughput level. `fc-home` reaches only 0.744 at b0 sustained: the ex-combiner's own clients are woken locally and win the next `try_lock`. Bursty, `home` is nevertheless faster (1.09--1.13× CES), because served waiters resume their parallel work on their own worker. _Bystander delay did not exceed the other workers' under balancing._ Our pre-registered hypothesis, a combiner-worker bystander p99 more than one pass above the others', is refuted in all 12 bursty cells for all five combining variants (e.g. 44.7 against 44.7 µs for CES at 8 workers, with a 136.6 µs pass). Without balancing the effect is starvation, not delay.

== Q2: Does usage-ordered combining restore service fairness? <sec:q2>

#fig("figures/fig-service.svg", wide: true)[Service fairness of FC-PQ, $W=8$, sustained, b31. (a) Pass limit $H$ (09-28 window): bars are measured service Jain, ticks the two-class model; dashed: goal G1. (b--f) Starvation-clamp sweep of `fcpq-h16-home`, newcomer init _mean_ (09-29 window): (b) service Jain with the model (ticks) and the confirmation cells (hollow; $W=8$ b0 and $W=16$ b31 were run at clamps 8 and 16 only); (c) light:heavy ops against the ratios the model needs for Jain 0.95 and 1; (d) throughput; (e) heavy-client run latency; (f) worst queue wait, with $c+1$ passes marked. Dashed green: same-window `fc-remote`. Medians of 3 repeats; error bars span [min, max].] <fig:service>

Yes, once the pass limit is below the backlog and the starvation clamp is too long to bind: `fcpq-h16-home-c16` reaches service Jain 0.997 with a 17-pass worst wait (@fig:service; @tab:service in the appendix). The pass limit decides whether ordering acts at all (@fig:service a): `fcpq` with $H=64$ admits every waiter in every pass and reaches only 0.706; $H=8$ gives 0.763, $H=16$ 0.863, and $H=16$ with `home` wakes 0.932. The other knobs change no admission set and stay at or below `fcpq`'s level (pass budget `t16000` 0.661, rotation 0.708, combining credit 0.693, max-usage election 0.707).

At $H=16$ with `home` wakes the 8-pass clamp binds: L:H stays at 3.64, as §@sec:policy predicts, below the #LHNinetyFive that Jain 0.95 needs (@fig:service c). Clamp 16 lifts service Jain to 0.997 at $W=8$ b31, $W=8$ b0 and $W=16$ b31 (@fig:service b; @tab:confirm), L:H to 5.45--5.53 and ops throughput by 13 % (0.540→0.612 Mops/s, same window); clamp 32 and no clamp change nothing further. The price is latency (@fig:service e, f): heavy p50/p99 grow from 253/357 µs to 328/626 µs, light p99 from 164 to 343 µs, and the worst wait is 17 passes at clamp 16, 33 at clamp 32 and 106#rng[54][193] with no clamp (heavy maxima 1.5--4.3 ms). We therefore take `fcpq-h16-home-c16`: the fairness of clamp-off with a bounded worst case (G2).

Two settings had no effect. The newcomer rule fires on only 64 of about 1.2 M requests per run, all during warm-up, so mean, zero, min and median initialisation are indistinguishable. In bursty load every knob is inert (Jain 0.697 at $W=8$): with at most 16 waiters and 7.8 ops per pass, every pass serves everyone pending. _Side effect on burden._ Clamp 16 also fixes a b0 burden problem, from 0.501#rng[0.500][0.544] to 0.970#rng[0.969][0.974]; our explanation [inference]: under clamp 8 the pass-end hand-off almost always goes to a light client, and at b0 light clients live on 4 of the 8 workers.

== Q3: Does fairness need delegation? <sec:qdel>

#fig("figures/fig-dpq.svg", wide: true)[Usage order without delegation (09-29 window). `dispatch-pq` is a hand-off mutex with FC-PQ's `UsageQueue`; colour gives its wake placement, fill its clamp in hand-offs (hatched: 16, FIFO-degenerate; solid: 256; dotted: off). (a) Service Jain; dashed: goal G1. (b) Lock utilisation. At $W=8$ b0 and $W=16$ only four variants ran; `fc-remote` has no same-window run there. (c) Per-op non-CS cycles $o$ in runs instrumented per hand-off ($W=8$ b31 sustained; 2.8--4.2 % slower): queue spinlock, queue operations (clamp scan, heap pop, grant) and grant to CS start (wake, run-queue wait, poll); for FC-PQ, in-pass administration plus pass gap. Dotted: $macron(C)$ of the fair mix. Medians of 3 repeats; error bars span [min, max]. Numbers: @tab:dpq.] <fig:dpq>

Not for fairness; but a hand-off realisation of the policy pays a scheduler round trip per decision. We ran `dispatch-pq` with default, home and remote wakes and clamps of 16 hand-offs, 256 and off, beside `dispatch`, `fc-remote` and `fcpq-h16-home-c16`, in one window (@fig:dpq; @tab:dpq in the appendix).

#par(first-line-indent: 0pt)[*Usage order is fair behind a hand-off.* With clamp 256 or off, `dispatch-pq` reaches service Jain #(DpqFairJainMin) in all 12 such cells, with L:H #(DpqFairLHLo)--#(DpqFairLHHi) sustained and no client or bystander starved (@fig:dpq a). With the default clamp of 16 hand-offs it is FIFO (#(DpqFifoJainLo)--#(DpqFifoJainHi)): every request is promoted and waits exactly 62 hand-offs, as §@sec:policy predicts. Heavy p99 is higher than FC-PQ's (685--864 against 566--626 µs sustained); light p99 is 164--186 µs, against FC-PQ's 343 µs at $W=8$ b31 and 134 µs at $W=16$ and b0.]

#par(first-line-indent: 0pt)[*But each fair hand-off is expensive.* Fair `dispatch-pq` completes #(DpqThrDispLo)--#(DpqThrDispHi)× `dispatch`'s ops/s sustained, because it serves more cheap operations, yet its utilisation is #(DpqUtilDispLo)--#(DpqUtilDispHi)× `dispatch`'s and #(DpqUtilFcLo)--#(DpqUtilFcHi)× FC-PQ's (#(DpqThrFcLo)--#(DpqThrFcHi)× in ops/s; best placement per cell #(DpqBestUtilFcLo)--#(DpqBestUtilFcHi)×; @fig:dpq b). With the same code and placement, usage order costs 0.67--0.74× the utilisation of its own FIFO-equivalent clamp 16; FC-PQ lost only 0.96× for the same mix shift. Bursty, it reaches #(DpqBurUtilFcLo)--#(DpqBurUtilFcHi)× FC-PQ's utilisation, where FC-PQ is itself unfair (Jain 0.697).]

#par(first-line-indent: 0pt)[*The cost is the round trip, not the queue.* In the instrumented runs every acquisition was a hand-off (@fig:dpq c). For the fair variants the queue spinlock takes #(DpqSpinLo)--#(DpqSpinHi) cycles and the queue operations #(DpqQueueLo)--#(DpqQueueHi) (upper bounds; the probes add 220--294 cycles to $o$). Grant to CS start takes #(DpqGtsLo)--#(DpqGtsHi) cycles, #(DpqGtsShareLo)--#(DpqGtsShareHi) % of $o$: the grantee still has to be woken, scheduled and polled. Fair `dispatch-pq`'s $o$ is #(DpqOLo)--#(DpqOHi) cycles, #(DpqOOverFcpqLo)--#(DpqOOverFcpqHi)× FC-PQ's #(FcpqOInst) (#(FcpqAdminInst) in-pass administration plus #(FcpqGapInst) gap), and the identity of §@sec:policy accounts for the whole gap: at `-home-c256`'s own mix, FC-PQ's $o$ would give utilisation 0.690, against FC-PQ's measured 0.684. Placement moves the round trip but does not remove it: with default wakes the grantee waits 5 582 cycles, behind the releaser's own 4 000-cycle parallel work [inference]; home wakes cut this to 3 885, remote wakes to 4 394.]

#par(first-line-indent: 0pt)[*Summary.* Usage-ordered fairness needs no delegation. On this executor a dispatched hand-off, which wakes the grantee through the scheduler, has #(DpqOOverFcpqLo)--#(DpqOOverFcpqHi)× FC-PQ's per-op lock cost, which leaves fair `dispatch-pq` at about half of FC-PQ's throughput and utilisation. Delegation is the fast realisation we measured, not a proven necessity: whether an inline hand-off (a CES-style `ces-pq`) closes the gap is untested #todo[`ces-pq` (usage-ordered CES successor, chain bound 64, `home` break) not built or measured].]

== Q4: How do fair delegation locks compare with tokio's? <sec:q3>

#fig("figures/fig-xrt.svg", wide: true)[Throughput against tokio 1.53.1, relative to `tokio-mutex` (LIFO slot on) in the same cell; log scale (09-28 window; coro locks b31). Hatched: `tokio-mutex` with the LIFO slot disabled, as off/on within a throwaway `tokio_unstable` build whose LIFO-on runs are 0.96--1.00× the release binary. Black ticks on the delegation bars: the same throughput divided by LIFO-off `tokio-mutex`. Numbers above bars: clients that completed no operation in 2~s. `fcpq-h16-home` is clamp 8 and counts cheap light ops, so it is not a like-for-like throughput reference. Medians of 3 repeats; error bars span the numerator's [min, max]. Numbers: @tab:xrt and @tab:xrtfair.] <fig:xrt>

Under sustained load burden-fair delegation beats `tokio::sync::Mutex` by #(SusOnLo)--#(SusOnHi)×; under bursty load most of its advantage is tokio's hand-off delay. We ported the workload unchanged to tokio's multi-thread runtime (`tokio-bench`: same key stream, costs, histogram and pinning; @tab:xrt and @tab:xrtfair in the appendix). The executor is not a weak baseline: coro `dispatch` runs at 0.90--0.98× `tokio-mutex`, and removing tokio's coop budget (`-unconstrained`) moves throughput by 0--1 %. Sustained, the burden-fair locks (blue and green bars) starve no client or bystander and keep FIFO-level service Jain (0.653--0.678, against 0.663--0.672 for `tokio-mutex`).

#par(first-line-indent: 0pt)[*The LIFO-slot caveat.* Bursty ratios of #(BurOnLo)--#(BurOnHi)× come mostly from the LIFO slot (§@sec:forms). Against `tokio-mutex` with the slot disabled (black ticks), `ces-k64-home` and `fc-remote` run #(BurOffLo)--#(BurOffHi)× bursty and #(SusOffLo)--#(SusOffHi)× sustained: the advantage that holds under either configuration is 1.5--1.6× sustained and about 1.2--1.4× bursty. *Blocking mutexes.* `std-mutex` and `parking-lot` beat every async lock at $W=8$ bursty (5.02× and 4.90×) only by seizing the workers: they serve 8 of the 16 clients and starve all bystanders. Elsewhere the delegation locks beat `std-mutex` 1.80--3.28×. *Missing comparison.* The two runtimes were only measured together in the 09-28 window; `fcpq-h16-home-c16` and `dispatch-pq` (09-29) have no same-window tokio reference #todo[same-window tokio baseline for `fcpq-h16-home-c16` and `dispatch-pq`].]

== Q5: Is a server task enough? <sec:q4>

#fig("figures/fig-actor.svg")[Actor control: throughput relative to `fc-remote` in the same cell, against burden Jain. Shaded: the goal (burden Jain $>= 0.9$ at $>= 0.95×$ `fc-remote`). Marker shape gives the cell. At $W=8$ b31 all runs are from the 09-29 window. In the b0 and $W=16$ cells the references are stored 09-28 cells, so the actor ratios there (hollow) cross windows and understate the actor by up to about 3 %. Error bars span burden Jain's [min, max].] <fig:actor>

No. Under sustained load each actor variant is either fast or burden-fair, never both (@fig:actor). Plain `actor` is burden-fair (0.990 sustained, 0.999 bursty at $W=8$ b31) only because balancing steals keep moving the server, and it runs 0.91× same-window `fc-remote` sustained and 0.54× bursty: served clients are woken into the server's queue ahead of the yielded server, so each pass waits for their parallel work [inference]. At b0 it is a one-worker system: burden 0.125, 0.59× sustained, 0.19× bursty, and a server-worker bystander p99 of 268--283 µs. `actor-inline` restores throughput (1.04× sustained, 0.96× bursty at $W=8$ b31), but under sustained load its server never parks, so no wake ever re-places it: burden is 0.179#rng[0.125][0.267] at $W=8$ and 0.612#rng[0.556][0.651] at $W=16$, and at b0 its worker's bystander p99 is 156 µs, about one 64-request pass. `ces-k64-home` and `fc-remote` reach the goal corner because they move the combiner role by construction; a server task would need the same explicit migration.

== Q6: What does fairness cost? <sec:q5>

#fig("figures/fig-util.svg")[Lock utilisation (CS cycles / window) against the per-op cycles not spent in a critical section, $o$, for FC-family locks at $W=8$, sustained, b31. Curves: the identity util $= macron(C) \/ (macron(C)+o)$ for the FIFO mix ($macron(C)$ of same-window `fc-remote`), the mix at the Jain-0.95 bound, and the clamp-16 mix. Purple points are FC-PQ settings. Filled: 09-28; hollow: 09-29. Star: the $o$ that utilisation 0.80 needs at Jain 0.95. Error bars span [min, max].] <fig:util>

Service fairness costs lock utilisation, and the per-op lock cost $o$ accounts for all of the loss. Usage ordering alone costs little in a combining lock: at heavy ratio 1, FC-PQ's throughput is within 0--3 % of FC. On 09-28, utilisation drops from #UtilFcOld (`fc`) to 0.725 (`fcpq-h16-home`); on 09-29, from #UtilFcRemoteAB (`fc-remote`) to #UtilCEight (clamp 8) and #UtilCSixteen (clamp 16), while a combiner is busy 98.8--98.9 % of the window (@fig:util). Each point sits on the curve of its own mix: FC-PQ moves right only a little ($o$) but drops to curves of cheaper mixes ($macron(C)$). Its cost is $o=#OCSixteen$ cycles/op, 96--97 % of it in-pass administration; same-window `fc-remote` has $o=#OFcRemoteAB$, and the lowest $o$ of any FC-family sustained cell is `fc` on 09-28 at turbo clock, $o=#OFcOld$. `dispatch-pq`'s fair points (Q3), with $o$ of #(DpqPlainOLo)--#(DpqPlainOHi) cycles and utilisation #(DpqFairUtilLo)--#(DpqFairUtilHi), lie far off the figure's axes.

At the fairness bound $J=0.95$ (L:H $= #LHNinetyFive$), the mix costs $macron(C)=#CbarNinetyFive$ cycles, so utilisation 0.80 requires $o <= #ONeededEighty$ cycles/op (the star in @fig:util), 39 % below `fcpq-h16-home` and 25 % below `fc-remote`; even $o=#OFcOld$ would give only #UtilAtOFcOld. FC-PQ's ops/s gain is therefore partly an accounting effect: the lock does less work per second while completing more operations. Our first candidate for its cost [inference] is home wakes (on 09-28, `fcpq-h16` with default placement had 769 admin cycles/op against 1 005 for `-home`); we have not broken FC-PQ's $o$ into drain, heap, rekey and wake costs #todo[breakdown of $o$ into drain / heap / rekey / home-wake cycles].

#par(first-line-indent: 0pt)[*Summary.* Placement restores burden fairness at CES throughput (Q1); usage ordering restores service fairness once the pass limit is below the backlog and the clamp does not bind (Q2), and does so behind a hand-off mutex too, at about half the utilisation (Q3). Sustained, fair delegation beats tokio's mutex in either configuration (Q4), and a server task cannot match it without explicit migration (Q5). Service fairness is paid for through the per-op lock cost (Q6).]

// ---------------------------------------------------------------------------
= Discussion and Limitations <sec:discuss>

#par(first-line-indent: 0pt)[*Always-runnable bystanders.* Steal-when-idle never fires; our balancing steal substitutes for it. With I/O-driven bystanders, idle stealing could rescue trapped tasks and move a never-parking actor server; we have not tested this. *A synthetic workload.* One data structure plus spins, closed-loop, with no idle clients, late joiners or application benchmark. *One machine, two clock windows.* Three repeats give min/max spreads, not confidence intervals; cross-window ratios are uncertain by about 3 %. *Our CES.* Every CES number is for our implementation of its description~@ces.]

#par(first-line-indent: 0pt)[*Clamp units.* A starvation clamp means something only in its lock's tick: clamps that are safe in FC-PQ passes make `dispatch-pq` FIFO when counted in hand-offs (§@sec:policy), and our hand-off clamps of 256 and off were chosen after an exploratory single-repeat pass showed this, not pre-registered. By the binding rule, Jain 0.95 / 0.99 would need a hand-off clamp $c gt.tilde 149 \/ 184$ [inference, untested] #todo[`dispatch-pq` clamps 150--220 hand-offs, to trace the predicted Jain frontier]. *Uncontended fast path.* In every measured cell every `dispatch-pq` acquisition was a hand-off. Its one-CAS fast path is exercised only by the unit tests (95--97 % fast in the sparse test), so its low-contention cost is unmeasured #todo[`dispatch-pq` at low contention, where the fast path is taken].]

#par(first-line-indent: 0pt)[*An inline usage-ordered hand-off.* A CES-style hand-off that resumes the minimum-usage waiter inline on the releasing worker (`ces-pq`, e.g. chain bound 64 with `home` break) might remove the round trip of §@sec:qdel without closure delegation; CES already matches `fc-remote` under FIFO. Until it is measured, delegation is the fast realisation we measured, not the only possible one. *Future work.* `ces-pq`; I/O-driven bystanders; actor-server migration; the unrun clamps; FC-PQ's $o$ below 703 cycles/op; real applications.]

// ---------------------------------------------------------------------------
= Related Work <sec:related>

#par(first-line-indent: 0pt)[*Scheduler-cooperative locks.* SCL~@scl names _scheduler subversion_ for OS threads and fixes it with usage accounting and lock-slice penalties. Its user-space u-SCL is the non-delegating usage-ordered lock: each thread runs its own critical section. `dispatch-pq` is, in effect, its async analogue (usage order and a starvation clamp, no lock slices), and in a cooperative executor its every decision costs a scheduler round trip. SCL's §7 already suggests usage-fair scheduling of delegated requests, so combining the two is not our claim. CFL~@cfl schedules lock occupation by cgroup share and priority, NUMA-aware. We share their goal of fair lock _time_; the executor adds the _where_: blocking, hand-off placement and combiner burden.]

#par(first-line-indent: 0pt)[*Delegation locks.* FC~@fc is starvation-free and linearizable and lists the number of consecutive combining rounds as a knob; TCLocks~@tclocks bound combining batches at 1 024 waiters for "long-term fairness", with no fairness metric. Neither measures a per-thread combiner share; we measure it and move it. The FC dispatcher poster~@fcdispatch rotates the dispatcher role, which mitigates burden, but does not measure the cost.]

#par(first-line-indent: 0pt)[*CES.* CES~@ces states that it is the first delegation-style lock for cooperatively scheduled tasks; the claim is theirs. It evaluates throughput only, at uniform CS cost. Its motivating observation, that a dispatched hand-off puts a scheduler round trip on the critical path, is what §@sec:qdel measures for a fair hand-off. We add chain bounds with break placement, the burden metric, and usage-ordered service.]

#par(first-line-indent: 0pt)[*Async runtimes and locks.* The tokio mutex~@tokio is a FIFO semaphore whose hand-off wake goes through the LIFO slot, a bounded inline-after-poll hand-off on the unlocker's worker [inference from its source, consistent with §@sec:forms]; its fairness scope is only the waiters~@tokio6049. `async-lock`~@asynclock relies on a 0.5 ms starvation hand-off, which never fires when the notified waiter is never polled.]

#par(first-line-indent: 0pt)[*Preemptive request scheduling.* Shinjuku~@shinjuku preempts every 5--15 µs to handle heterogeneous request costs and places contended locks out of scope; Perséphone's DARC~@persephone reserves cores per request type. Both handle cost heterogeneity in the request scheduler; we handle it inside the lock, where a cooperative executor has no preemption to fall back on.]

// ---------------------------------------------------------------------------
= Conclusion <sec:concl>

In this paper, we have shown that a lock in a cooperative executor is a hidden scheduler that decides who runs next and where, and that at its defaults it subverts the executor in five ways: FIFO service in proportion to cost, monopoly, worker seizure, hand-off delay, and, for delegation, combiner burden. Service fairness is a lock-agnostic policy: one usage-ordered queue gives service Jain #(DpqFairJainMin) behind a hand-off mutex and 0.997 inside a combining lock, against 0.65--0.66 for FIFO. On our executor each fair hand-off pays a scheduler round trip, leaving the hand-off mutex at about half of the combining lock's utilisation. With wake placement against combiner burden (burden Jain 0.98--1.00), delegation is the fast realisation we measured, 1.5--1.6× `tokio::sync::Mutex` under sustained load; whether an inline hand-off matches it is open. Our hope is that runtimes will let locks choose where their wakes land, so that the scheduler hidden in every lock can cooperate with the one outside it.

#[
#set text(size: 9pt)
#bibliography("refs.bib", style: "ieee", title: "References")
]

// ---------------------------------------------------------------------------
#counter(heading).update(0)
#set heading(numbering: "A.1")
= Supplementary Tables <sec:appendix>

The tables below give the numbers behind @fig:service, @fig:dpq and @fig:xrt. They are generated by `make_tables.py` from the same result files; the per-cell burden, motivation-matrix and actor tables are in `paper/tables/`.

#tbl(size: 7pt, "tables/tab-dpq.typ", wide: true, colsep: 4pt, placement: bottom)[Usage order without delegation, the data of @fig:dpq a, b (09-29 window, one same-window run set). `dispatch-pq` rows give wake placement and clamp in hand-offs (c16 is the default; c0 is off). "util (×pq)" is lock utilisation and its ratio to `fcpq-h16-home-c16` in the same cell. Medians of 3 repeats; spreads in FINDINGS.md.] <tab:dpq>

#tbl(size: 7pt, "tables/tab-xrt-thr.typ", wide: true, colsep: 3.5pt, placement: bottom)[Throughput against tokio 1.53.1 (09-28 window; coro rows b31), the numbers of @fig:xrt. ×tm is the ratio to `tokio-mutex` in the same cell. *The bursty ratios of 4--5× are largely due to tokio's LIFO slot.*] <tab:xrt>

#tbl(size: 7pt, "tables/tab-xrt-fair.typ", wide: true, colsep: 3.5pt)[Service Jain and starved clients/bystanders for the runs of @tab:xrt (and @fig:motiv b). `std-mutex` and `parking-lot` are really $W$ pinned threads on a blocking mutex (§@sec:forms).] <tab:xrtfair>

#tbl(size: 7pt, "tables/tab-service.typ", wide: true)[Service fairness, $W=8$, sustained, b31. Upper block: phase 3 (09-28). Lower block: same-window A/B and clamp sweep (09-29, 3.0 GHz cap); `-c`$N$ is `fcpq-h16-home` with starvation clamp $N$ (0 = off) and newcomer init _mean_. "model $J$" is the two-class model applied to the measured L:H and per-class costs. L:H, util and non-CS/op are medians (max spread 5 % of the median). util $=$ CS cycles / window. non-CS/op $=$ (window $-$ CS cycles) / ops; only for the FC-family rows, whose combiner is busy almost the whole window, is it the per-op lock cost $o$ of §@sec:design; for `dispatch` and `ces` it also contains lock-idle and hand-off time. Max wait is in combining passes. Do not compare rows across the two blocks.] <tab:service>

#tbl(size: 7pt, "tables/tab-clamp-confirm.typ", colsep: 3pt, placement: none)[Clamp 8 vs. 16 for `fcpq-h16-home` in the confirmation cells (09-29 window). Heavy p99 is run latency in µs; L:H, p99 and burden $J$ are medians.] <tab:confirm>

#tbl(size: 7pt, "tables/tab-loc.typ", placement: bottom)[Lines of Rust: non-blank, non-comment lines, with lines from the first `#[cfg(test)]` on counted as tests. Counted by `make_tables.py`.] <tab:loc>

#tbl(size: 7pt, "tables/tab-lifo.typ", wide: true, colsep: 2.5pt, placement: bottom)[tokio LIFO slot on/off (Mops/s; throwaway `tokio_unstable` build whose LIFO-on runs are 0.96--1.00× the release binary; 09-28 window): @fig:motiv c and the hatched bars and black ticks of @fig:xrt. The right-hand columns divide the coro throughputs of @tab:xrt by LIFO-off `tokio-mutex`.] <tab:lifo>
