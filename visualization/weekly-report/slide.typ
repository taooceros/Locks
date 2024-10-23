#import "@preview/touying:0.5.2": *
#import "@preview/codly:1.0.0": *
#import "@preview/chronos:0.1.0"

#show: codly-init.with()
#import themes.dewdrop: *

#show: dewdrop-theme.with(aspect-ratio: "4-3", navigation: none)
#show link: set text(blue.darken(20%))

#set heading(numbering: "1.") 


= Recap: Delegation Styled Lock

#table(columns: (30%, 70%), stroke: none, gutter: 2em, [
    #set par(linebreaks: "optimized", justify: false)
    #v(3em)

    Thread publish their critical section to a job queue, and one server thread (combiner) will execute the job to prevent control flow switching.



    Two aspects:
    1. How to elect the combiner thread
    2. How to schedule the job
  ], [
  #image("Delegation Styled Lock Illustration.svg", width: 100%)
])

== Imbalanced Workload

#table(columns: (20%, 75%), stroke: none, gutter: 2em, [
  #set par(linebreaks: "optimized", justify: false)
  #v(5em)
  $t_2$ is occupying more lock-usage than $t_1$ and $t_3$
  ], [
  #image("Imbalanced Workload DLock.svg", width: 100%)
])

#pagebreak()

=== Lock Usage Fairness

- $t_2$ uses the lock longer than $t_1$ and $t_3$ because it has a longer critical section.
- $t_1$ can leave the lock earlier if using a normal lock, since when it acquires the lock, the lock is uncontended. However, now it needs to help other threads to execute the critical section.

== Scheduler Subversion

- Assuming the threads are over-subscribed #footnote[This is actually valid assumption, since delegation styled lock is much more scalable than normal lock which enable the potential of very large number of threads], scheduler will pre-empt the threads.
- Assuming instead of spin-waiting, threads are sleeping during waiting #footnote[Consider the case when we needs very large number of threads].

+ In a normal lock, Scheduler Subversion happens when threads holding shorter critical section are spinning "longer", which consumes CPU time but not progressing, while threads that holding longer critical section are using the lock more proportionally. This subverts the fairness goal provided by the scheduler.
+ In delegation styled lock, the combiner thread is helping the other threads. Since this delegation is transparent to the scheduler, scheduler will panelize the combiner thread because of its voluntary work.

*Example*
+ When $t_1$ is helping $t_2$ and $t_3$, it will use more CPU time, but not doing its own job.
+ Scheduler is not aware of the delegation, so it will try to schedule $t_1$ less time because of its voluntary work.

= Solutions to Lock Usage Fairness

+ Banning
+ Priority Queue (CFS like)
+ Other Scheduling Mechanism

== Banning

- Similar to U-SCL, we ban threads that executes long critical section.

=== Implementation

- Flat Combining with Banning (@flat-combining-banning)
- CCSynch with Banning (@ccsynch-banning)

=== Problem

- If there are threads that enters the lock sparsely, there may be chance that all current contending threads are banned, while the lock remains unused.

== Naive Priority Queue

- Use a priority queue as the job queue (e.g. skip-list).

=== Procedure

- Combiner elected via an `AtomicBool`
- Priority queue is implemented via skip-list (#link("https://docs.rs/crossbeam-skiplist/latest/crossbeam_skiplist/", [crossbeam-skiplist]))
- To execute a critical section, thread post the request to a local node and pushes it into the skip-list.
- Combiner will pop the job from the skip-list.

=== Implementation

+ @fc-sl

=== Illustration

TODO

=== Problem

- Performance overhead of skip-list is really high.
- Dequeue is expensive, which waste combiner's CPU time (i.e. wasting potential lock usage).

== Priority Queue (CFS like)

- I want to propose something that is easy to implement, which allows more general scheduling mechanism.

=== Motivation

- Combiner has exclusive control over the lock-usage statistics (why do we need distributed ordering).
- We may want other scheduling mechanism, e.g. EEVDF.
- Node can be reused

=== Idea

- Use something like an MPSC channel to publish the job.
- The combiner thread polls the channel to get the job, and re-order the job based on exectuion.

=== Illustration

TODO

=== Challenges

TODO!

1. Deadlock of a naive implementation (TODO).
  1. Workaround: TODO
2. How to elect the combiner thread (subversion problem)?
3. Publishing node can be expensive
  1. Caching?
4. When do combiner check the channel?

=== Implementation

- @fc-pq


= Implementation

== Flat Combining

A singlely linked list of nodes (belongs to each threads) are used to publish the job.

#image("flat-combining.png")

#pagebreak()

=== Illustration 1

#pagebreak()

=== Illustration 2

#let circle_num(number) = numbering("①", number)

#chronos.diagram({
  import chronos: *
  let t1_seq = _seq.with(color: green.darken(20%), lifeline-style: (fill: green))
  let t2_seq = _seq.with(color: red, lifeline-style: (fill: red))
  _par("Lock")
  _par("Head")
  _par("T1 Node")
  _par("Combiner")
  t1_seq("Lock", "Lock", comment: [T1 start])
  t2_seq("Lock", "Lock", comment: [T2 start])

  t2_seq("Lock", "T2 Node", create-dst: true, dashed: false, comment: [#circle_num(1)])
  t2_seq("T2 Node", "Lock", dashed: false)

  t1_seq("Lock", "T1 Node", create-dst: true, dashed: false, comment: [#circle_num(1)])
  t1_seq("T1 Node", "Lock", dashed: false, disable-src: true)

  t1_seq("Lock", "Combiner", comment: [#circle_num(2) try_lock(true) #footnote[A "lock" inside the fc_lock that is used to provide mutual exclusion for combiner] <fc_inner_lock>], enable-dst: true)
  t1_seq("Combiner", "Head", comment: [#circle_num(3) combine])
  t1_seq("Head", "T1 Node", comment: [#circle_num(3)], enable-dst: true)

  t1_seq("T1 Node", "T1 Node", comment: [completed = true])
  t1_seq("T1 Node", "T2 Node", comment: [#circle_num(3) Next])

  t2_seq("Lock", "T2 Node", comment: [#circle_num(2) try lock(false)], create-dst: false)


  t2_seq("T2 Node", "T2 Node", comment: [Wait Completed (with timeout)], enable-dst: true)
  
  t1_seq("T2 Node", "T2 Node", comment: [completed = true])

  t1_seq("T2 Node", "Combiner", comment: [Finish #circle_num(3)])

  t2_seq("T2 Node", "Combiner", comment: [(timeout) #circle_num(2) try lock (true) #footnote[Dashed Line means potentially executed but not in this case.]], enable-dst: false, dashed: true)
  t2_seq("T2 Node", "T2 Node", comment: [read completed = true], disable-dst: true)
  t1_seq("Combiner", "Combiner", comment: [#circle_num(4) cleaning (if needed)])
  t1_seq("Combiner", "Lock", comment: [#circle_num(5) unlock #footnote(<fc_inner_lock>)], disable-src: true)
  t2_seq("T2 Node", "Lock", comment: [#circle_num(5) completed = true $=>$ return])
}, width: 70%)

== CCSynch

_CCSynch_ maintains a FIFO queue of the job.

#enum(numbering: "①")[
  Acquire a _next_ node from `thread_local`
][
  Swap the `Tail` with the `next` node
][
  Wait `wait`
][
  if `!completed`, traverse the queue and execute jobs, set `completed` to `true`, and `wait` to `false`
][
  when reaching combine limit, set next `wait` to `false`, and `completed` to `false`
]


#align(center)[
  #image("ccsynch.drawio.svg", width: 80%)
]

#chronos.diagram({
  import chronos: *

  let t1_seq = _seq.with(color: green.darken(20%), lifeline-style: (fill: green))
  let t2_seq = _seq.with(color: red, lifeline-style: (fill: red))
  _par("Lock")
  _par("T1")
  _par("T2")
  _par("T1 Data")
  _par("T2 Data")

  t1_seq("T1", "Lock", comment: [start], enable-dst: true)

  t1_seq("Lock", "T1 Data", create-dst: true, comment: [#circle_num(1) Get Node])
  t1_seq("T1 Data", "Node 1", comment: [#circle_num(2) Swap `Tail`], create-dst: true)

  t1_seq("Node 1", "Node 1", comment: [#circle_num(3) Wait])
  t2_seq("T2", "Lock", comment: [start], enable-dst: true)

  t1_seq("Node 1", "Node 1", comment: [Check Completed])

  t1_seq("Node 1", "Combiner", comment: [#circle_num(4) If not $=>$ Combine], enable-dst: true)

  t2_seq("Lock", "T2 Data", comment: [Acquire Node])
  t2_seq("T2 Data", "Node 2", comment: [Swap with `Tail`], create-dst: true)
  t2_seq("Node 2", "Node 2", comment: [Wait], enable-dst: true)

  t1_seq("Combiner", "Node 1", enable-dst: true, comment: [Traverse queue])
  t1_seq("Node 1", "Node 1", comment: [completed=T, wait=F])
  t1_seq("Node 1", "Node 2", comment: [Execute Next], disable-src: true, enable-dst: true)

  t1_seq("Node 2", "Node 2", comment: [
    completed=T, wait=F
  ], disable-src: true)
  t2_seq("Node 2", "Lock", comment: [Return], disable-dst: true)
  t1_seq("Node 2", "Combiner", disable-dst: true, disable-src: true)
  t1_seq("Combiner", "Lock", comment: [Return], disable-dst: true)
}, width: 80%)

== Flat Combining with Banning <flat-combining-banning>

TODO!

== CCSynch with Banning <ccsynch-banning>

TODO!

== FC-PQ <fc-pq>

TODO!

== FC-Skiplist <fc-sl>

TODO!

= Profiling Result

== Flat Combining

#let fc-profile = csv("fc-profile-1.csv")

#[
  #set text(size: 20pt, hyphenate: false)
  #set par(linebreaks: "optimized")

  #table(
    columns: (auto, auto, auto, auto, auto, auto),
    inset: 5pt,
    align: center,
    stroke: none,
    table.hline(),
    [*Function*], [*CPU Time*], [*Clockticks*], [*Instructions Retired*], [*CPI Rate*], [*Module*], 
    table.hline(),
    [lock], [44.490s], [$1.11 times 10^11$], [$1.30 times 10^9$], [85.489], [dlock],
    [bench code], [4.682s], [$1.17 times 10^10$], [$1.27 times 10^10$], [0.918], [dlock],
    [[vmlinux]], [0.069s], [$1.49 times 10^8$], [$6.27 times 10^7$], [2.368], [vmlinux],
    [thread::current], [0.037s], [$9.68 times 10^7$], [$1.98 times 10^7$], [4.889], [dlock],
    table.hline(),
  )

  #v(1em)

  #table(
    columns: (auto, auto, auto, auto, auto),
    inset: 5pt,
    align: center,
    stroke: none,
    table.hline(),
    [*Function*], [*Retiring*], [*Front-End Bound*], [*Bad Speculation*], [*Back-End Bound*],
    table.hline(),
    [lock], [0.90%], [0.10%], [0.00%], [99.10%],
    [bench code], [30.90%], [0.30%], [0.80%], [68.00%],
    [[vmlinux]], [34.30%], [57.20%], [11.40%], [0.00%],
    [thread::current], [87.80%], [100.00%], [0.00%], [0.00%],
    table.hline(),
  )
]


== CCSynch

== FC-PQ-BHeap


== Mutex

= Code Change

== Shared Counter

+ Remove `blackbox` for accessing the data
+ Change the blackbox position

```diff
- while black_box(loop_limit) > 0 {
-   *data += 1;
- }
+ while loop_limit > 0 {
+   *black_box(&mut *data) += 1;
+   loop_limit -= 1;
+ }
```

#pagebreak()

=== Reason for `blackbox`

+ `loop_limit` => the length of *Critical Section*.
+ Compiler will optimize the code to something like `*data += loop_limit;`, which will make varying the `loop_limit` not affecting the length of *Critical Section*.

=== Reason for the change

+ I want to mimic more access to the shared variable (hopefully something like `inc (rax)` in assembly).
+ The previous version contains too much overhead for doing the loop.