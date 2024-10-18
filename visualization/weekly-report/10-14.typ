#import "@preview/touying:0.5.2": *
#import "@preview/codly:1.0.0": *
#import "@preview/chronos:0.1.0"

#show: codly-init.with()
#import themes.dewdrop: *

#show: dewdrop-theme.with(aspect-ratio: "4-3", navigation: none)

= Recap Implemented Locks

== Flat Combining

Precondition: Each lock has a singly linked list of nodes belongs to each thread.

#image("flat-combining.png")


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

== Flat Combining with Banning

== CCSynch with Banning

== FC-PQ

== FC-Skiplist

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