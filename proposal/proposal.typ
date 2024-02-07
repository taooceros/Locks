#import "template.typ": *
#import "@preview/wordometer:0.1.0": word-count, total-words

#show: project.with(
  title: "Usage-Fairness in Delegation-Styled Locks",
  authors: ("Hongtao Zhang",),
  advisor: "Remzi Arpaci-Dusseau",
)

#show link: it=> text(rgb("0000EE"))[#it]

#heading("Abstract", numbering: none)

#set par(leading: 0.6em)

#let fc = [_Flat-Combining_]
#let ccsync = [_CC-Synch_]
#let hsynch = [_H-Synch_]
#let dsmsync = [_DSM-Synch_]
#let RCL = [_RCL_]
#let ffwd = [_ffwd_]

#word-count(
  total => [
    This proposal outlines a plan to investigate the efficacy of delegation-styled
    lock and the combination with the idea of delegation and usage-fairness.
    Previous studys has shown the problem of scheduler subversion when locks only
    adapt fairness in acquisition-level, in which most delegation-styled lock
    provides. They propose that lock usage (the amount of time holding the lock)
    should be viewed as resource as cpu time slice and fairness gurantee need to be
    provided. We propose to modify the state of art combining locks (#fc, #ccsync, #hsynch, #dsmsync)
    and client-sever locks (#RCL, #ffwd) to adapt usage-fairness principle with a
    simple strategy, "banning" to ensure the proporsional share of lock usage among
    threads. Beyond that, delegation-styled locks where combiner is elected on the
    fly sacrifice the response time of the combiner, as it is doing volunteer work
    for other threads. We propose to employ stochastic methods to ensure the
    volunteer work proporsional to lock usage. Beyond that, we plan to redesigned
    combining strategy that are native with usage-fairness principle based on a
    concurrent MPSC Priority Queue. The performance benchmark of these locks will be
    performed in various _Concurrent Object_ (Fetch&Add, Fetch&Multiply, Queue,
    PriorityQueue, etc.) with response time analysis.
     
    #h(1fr) #total.words
  ],
)

#set par(leading: 1.5em)

= Introduction

In modern computer industry, the focus of improving of the Central Process Unit
(CPU) has shifted from increasing the clock frequency to increasing the number
of cores. This shift has led to the development of multi-core processors, which
are now widely used in many computer systems. The scalability of applications on
these multi-core machines are captured by _Amdahl's Law_, stating that the
maximum speedup of a program is limited by the fraction of the program that
cannot be parallelized.

One of the most common place where parallelization is hard is when different
threads are communicating with each other via shared data. One of the most
common strategy used to synchronize concurrent programs is the use of locks,
which provides the gurantee of mutual exclusion @ostep_ref @amp_ref. Since
synchronization must be executed in mutual exclusion, their execution becomes a
hot spot in various concurrent environment @ccsynch_ref. Ideally, the time to
execute the same number of synchronization should be the same regardless the
number of threads. However, in practice, threads that are contenting for a lock
can drastically impact the performance of the system.

_Delegation-styled lock_ is a new class of locks that aims to reduce the
contention and data movement on the lock by delegating the work to one threads.
In this technique, instead of having all threads to compete for the lock, each
threads will wrap their critical section in a request and send it to a combiner.
The combiner will then execute the request and return the result to the threads.
This technique has been shown to outperform traditional locks in various
circumstances. There are two main classes of delegation-styled locks: _combining_ synchronization
@ccsynch_ref @transparent_dlock_ref @flatcombining_ref and _client-server_ synchronization
@rcl_ref @ffwd_ref. The former each participant acts as combiner temporarily,
while the latter has a fixed thread as combiner. Specifically, resent work has
demonstrate the potential of employing a delegation-styled lock with traditional
lock api, which open the potential of using delegation lock in large system that
is hard to modify @transparent_dlock_ref.

However, earlier study has introduce the problem of scheduler subversion when
locks do not adapt fairness or only in acquisition-level, in which most
delegation-styled lock provides @scl_ref. When thread has inbalanced workload in
their critical sections, the present of lock will subvert the scheduling policy
of CPU provided by the operating system, in which both threads should have the
same amount of CPU time. This problem is crucial when some threads require low
latency with minimal access to the lock, such as a thread handling user
interactive work, while others requires large computation with the data, while
the competion of the lock will subvert the original goal of CPU scheduler
ensuring low latency for short work. Furthermore, this issue is more severe in
the delegation-styled lock where thread will temporarily act as combiner. For
example, if the thread handling the interactive work is elected as combiner,
user may experience serious latency issue, which causes combining lock less
attractive in the system.

To remedy this problem, we propose to adapt a similar strategy employed in the
original paper, "banning" to ensure lock usage fairness for these
delegation-style locks. Furthermore, we plan to design our own combining
strategy that are native with usage-fairness principle based on a concurrent
MPSC Priority Queue. We will also employ stochastic methods to share the
combining evenly.

= Method

All the experiment will be performed on #link("https://www.cloudlab.us/", [Cloudlab]),
a large-scale testbed for cloud research that provides researchers with control
and visibility all the way down to the machine.

The implementation of these locks will be programmed in _rust_, a modern systems
programming language that is designed to be safe, concurrent, and performant.
The performance benchmark of these locks will be performed in various _Concurrent Object_ (Fetch&Add,
Fetch&Multiply, Queue, PriorityQueue, etc.) with response time analysis. The
performance of these locks will be compared with the original implementation and
the state of art locks.

Datas will be stored in #link("https://arrow.apache.org/", [Apache Arrow]) format,
and the analysis will be done in #link("https://julialang.org/", [Julia]) with
the help of #link("https://dataframes.juliadata.org/stable/", [DataFrames.jl]) and #link("https://arrow.apache.org/julia/stable/", [Arrow.jl]),
where plots will be drawn with #link("https://docs.makie.org/stable/", [Makie]) @makie_ref with #link("https://aog.makie.org/stable/", [Algebra Of Graphics]).

== Performance Counter

= Timeline

The task of this project is majorly broken down into the following parts

+ Implementation of various delegation-styled locks in _rust_.
  + _Flat-Combining_ @flatcombining_ref (Done)
  + _CC-Synch_ @ccsynch_ref (Done)
  + _H-Synch_ @ccsynch_ref (March, 2024)
  + _DSM-Synch_ @ccsynch_ref (April, 2024)
  + _RCL_ @rcl_ref (Done)
  + _ffwd_ @ffwd_ref (April, 2024)
+ Employing banning strategy to ensure usage-fairness principle in
  delegation-styled locks.
  + _Flat-Combining_ @flatcombining_ref (Done)
  + _CC-Synch_ @ccsynch_ref (Done)
  + _H-Synch_ @ccsynch_ref (April, 2024)
  + _DSM-Synch_ @ccsynch_ref (September, 2024)
  + _RCL_ @rcl_ref (Done)
  + _ffwd_ @ffwd_ref (September, 2024)
+ Employing stochastic methods to ensure volunteer work is shared evenly (October,
  2024)
+ Redesigning combining strategy that are native with usage-fairness principle
  based on a concurrent MPSC Priority Queue (October, 2024)
  + Implementation based on existing concurrent MPSC Priority Queue (October)
  + Design a new (probably relaxed) concurrent MPSC Priority Queue (November)
  + Implementation of the new concurrent MPSC Priority Queue (November)
+ Implementation of benchmark suites
  + Fetch&Add (Done)
  + Fetch&Multiply (Done)
  + Queue (Feburary)
  + PriorityQueue (March)
  + Inbalance Workload Data Structure (September)
+ Performance analysis Suite 
  + Throughput/Scalability (Done)
  + Fairness (March)
  + Response Time (April)
+ Performance analysis (On the fly)

= Conclusion

In conclusion, we propose to demonstrate that delegation-styled locks suffers
from the scheduler subversion problem. To remedy this problem, we propose to
integrate existing delegation-styled locks with "banning" strategy to ensure
their usage fairness. Further we propose to employ stochastic methods to share
the combining evenly.

In the future, we plan to resolve the response time issue of the combiner by
swapping the non-critical work of combiner to one of the waiter that expected to
wait long. One possible approach is to employ a similar strategy used in the TCL
Lock @transparent_dlock_ref, while another proposal is to embrace the
asynchronous programming model provided by _rust_ to delegate the
#raw("Future") for execution and create a custom runtime that adapts takes
lock-usage.




#set par(leading: 0.6em)
#bibliography("literature.yml", style: "acm-sig-proceedings.csl")