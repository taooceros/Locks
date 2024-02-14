#import "template.typ": *
#import "@preview/wordometer:0.1.0": word-count, total-words

#show: project.with(
  title: "Usage-Fairness in Delegation-Styled Locks",
  authors: ("Hongtao Zhang",),
  advisor: "Remzi Arpaci-Dusseau",
)

#show link: it=> text(rgb("0000EE"))[#it]

#heading("Abstract", numbering: none)

#set par(leading: 0.5em)

#let fc = [_Flat-Combining_]
#let ccsync = [_CC-Synch_]
#let hsynch = [_H-Synch_]
#let dsmsync = [_DSM-Synch_]
#let RCL = [_RCL_]
#let ffwd = [_ffwd_]

#let indent = h(1.5em)

#word-count(
  total => [
    #indent This proposal presents a comprehensive plan to explore the
    effectiveness of a novel delegation-styled locking mechanism that
    integrates the concepts of delegation and usage fairness. Prior research
    has identified challenges with scheduler subversion arising from locks that
    adapt a no or strictly acquisition-level fairness, which is common in
    current delegation-styled locks. Furthermore, some delegation locks will
    elect a combiner from participants, which often compromise the combiner's
    latency due to the additional workload it assumes for other threads. They
    suggest treating lock usage as a resource, akin to CPU time slices,
    warranting a usage-level fairness. I aim to enhance state-of-the-art
    combining locks (#fc, #ccsync, #hsynch, and #dsmsync) and client-server
    locks (#RCL and #ffwd) by incorporating a usage-fairness principle. The
    straightforward "banning" strategy will be implemented to ensure a
    proportional allocation of lock usage time across threads. A stochastic
    methods will be employed to proportionally distribute the voluntary
    workload based on lock usage. In addition, I plan to devise a new
    scheduling strategy inherently aligned with the usage-fairness principle by
    leveraging a concurrent relaxed Priority Queue. The efficacy of these
    enhanced locking mechanisms will be tested through micro-benchmarks on
    various commonly used Concurrent Objects complemented by an in-depth
    latency analysis.
     
    #h(1fr) #total.words
  ],
)

#set par(leading: 1.5em)

= Introduction

#indent In the current landscape of computational technology, the
imperative to enhance Central Processing Unit (CPU) performance has
transitioned from escalating clock speeds to multiplying core counts. This
evolution has given rise to multi-core architectures, which have become
ubiquitous across computer systems. The scalability of applications on such
multi-core infrastructures is predicated on Amdahl's Law, which postulates
that the theoretical maximum improvement achievable through parallelization
is limited by the code that must remain sequential.

A principal challenge in parallel computing is thread coordination via
shared resources. Lock-based synchronization mechanisms are widely employed
to ensure mutual exclusion and are critical for threads to communicate
accurately and reliably @ostep_ref @amp_ref. These synchronization points,
however, are often a source of contention and can become performance
bottlenecks in a concurrent execution environment @ccsynch_ref.
Theoretically, the synchronization duration should be invariant with
respect to the number of threads; yet, contention for locks often leads to
a serious degradation in performance that is disproportionate to the
increase in thread count @ccsynch_ref @flatcombining_ref @amp_ref.

_Delegation-styled_ locks have emerged as a innovative solution aimed at
boosting synchronization efficiency by minimizing contention and the
associated overhead of data movement. Instead of each thread compete for a
lock to execute their critical section, threads package their critical
sections into requests and entrust them to a combiner, which processes
these requests and returns the results. There are two predominant forms of
delegation-styled locks: _combining_ synchronization @ccsynch_ref
@transparent_dlock_ref @flatcombining_ref and _client-server_ synchronization
@rcl_ref @ffwd_ref. Combining locks allow for dynamic selection of the
combiner role amongst the participants, whereas client-server locks
dictates a consistent server thread to manage all requests. Empirical
evidence suggests that this technique can outperform traditional locking
mechanisms, even approaching the ideal of sequential execution efficiency
regardless of number of threads. 

// Despite their advanced design, delegation-styled locks have been criticized
// for their complexity and difficulty in integration with complex
// applications. Recent advancements, however, have demonstrated the
// feasibility of melding delegation-styled locks with conventional lock APIs
// with _transparent delegation_, thereby broadening their potential for
// deployment in extensive systems @transparent_dlock_ref.

Newly conducted studies have introduced concerns regarding scheduler
subversion when locks are implemented without a sophisticated fairness
mechanism or are limited to fairness at the point of acquisition @scl_ref.
This is particularly problematic when threads exhibit imbalanced workloads
within their critical sections, as the presence of a lock can disrupt the
CPU's scheduling policy, which intends to allocate equitable processing
time to concurrent threads. Envision a scenario where interactive threads
engaging with users are in contention with batch threads performing
background tasks, all synchronized by a lock. Absent a principle of usage
fairness, the interactive threads may suffer from inordinate delays in lock
acquisition, thereby subverting the CPU scheduler's objective of ensuring
prompt response times for interactive tasks. Moreover, the issue is
magnified in the context of delegation-styled locks, where the elected
combiner thread may be burdened with an unequal share of work. If an
interactive thread is chosen as the combiner, it could lead to severe
latency issues for the user, thus diminishing the attractiveness of
combining locks in systems with disparate workloads.

To remedy this problem, I propose to integrate existing delegation-styled
with the concept of _usage-fairness_ by employing the banning strategy
@scl_ref. By restricting access to the lock according to their usage, we
can prevent any single thread from monopolizing CPU resources, thus
upholding the principle of equitable computational opportunity amongst
concurrent processes.

= Method

== Implementation

#indent I propose a simple heuristic strategy, "banning", inspired by SCLs
to remedy the unfairness of delegation-styled locks, by restricting thread
that recently enter the lock to reenter the lock @scl_ref. Specifically,
every threads will be banned with a heuristic algorithm based on their
critical section length. Formally, a thread is banned from reacquiring the
lock for a duration calculated by the expression: $n_"thread" times c s - c s_"avg"$,
where $c s$ refers to the critical section length of the thread and $c s_"avg"$ is
the average length of critical section accross threads that trying to
acquire the lock. This methodology promises an equitable distribution of
lock usage among threads over time given the assumption that all threads
are actively contenting for the lock.

To relaxing the assumption, I propose to engineer a bespoke combining
strategy that adheres to the usage-fairness principle analogous to the CFS
(Completely Fair Scheduler) employed in the Linux kernel. The combiner will
prioritize tasks that have consumed the least amount of lock usage, much
like the CFS selects tasks with the minimum time slice used.

== Experiment

#indent Experiments will be conducted on #link("https://www.cloudlab.us/", [Cloudlab]),
a sophisticated cloud research testbed that provides comprehensive control
and visibility down to the bare metal. The implementation of these locks
will be developed in _rust_, a modern system programming language known for
its safety, concurrency, and performance. The performance benchmark of
these locks will be conducted on commonly used _Concurrent Objects_. The
key performance metric will include throughput, latencies, and fairness.
Datas will be stored in #link("https://arrow.apache.org/", [Apache Arrow]) format,
and the analysis will be done in #link("https://julialang.org/", [Julia]) with #link("https://dataframes.juliadata.org/stable/", [DataFrames.jl]) and #link("https://arrow.apache.org/julia/stable/", [Arrow.jl]),
where plots will be drawn with #link("https://docs.makie.org/stable/", [Makie]) @makie_ref with #link("https://aog.makie.org/stable/", [Algebra Of Graphics]).

=== Benchmark Suite

#indent The benchmark suite will be implemented in _rust_ and will be
open-sourced. #raw("rdtscp"), a special time stamp counter in x86_64
instruction set, is used to measure the time for micro-benchmark. The
benchmark suite will record the following execution data for a set of
commonly used concurrent object: `thread_num` (the number of threads that
is concurrently accessing the concurrent object), `operation_num`,
`latencies` (how long each operation takes to start), `self-handled`
(whether operation is performed by the same thread), `hold_time` (the total
time the thread is using the concurrent object), `combine_time` (the total
time the thread is performing volunteering work), `noncs_length` (a time
slice where the thread doesn't touch the concurrent object). #footnote[Some of the data will only valid for a subset of the concurrent objects.]

=== Concurrent Object

#indent I aim to evaluate the efficacy of the proposed locks using the
following concurrent objects:

+ `Fetch&AddLoop`: a synthetic benchmark will help in assessing the
  performance characteristics of the locks across different workloads.
+ `Fetch&Multiply`: a short operation that no hardware offers direct atomic
  support.
+ `Queue` and `PriorityQueue`: Common concurrent data structures with
  significant relevance in concurrent applications. They are also avaliable
  lock-freely.

=== Performance Metrics


+ *Throughput*: The number of operations that can be performed in certain
  time slice. This metric shows the bulk performance of the concurrent
  object. For `Fetch&AddLoop`, this is equal to the number of times through
  each loop, while for other concurrent objects, this is equal to the number
  of operations performed.
+ *Latency*: The time it takes for each operation to start, capturing the
  response time.
+ *Lock Usage Fairness*: This metric follows follows the original definition
  of _Lock Oppourtunity_ and _The Fairness Index_ capturing the idea of
  fairness of the lock among all threads @scl_ref.

#indent I will examine the scalability of the locks by analyzing throughput
in relation to an increasing number of threads. Fairness will be evaluated
by considering both the duration each thread retains the lock (`hold_time`)
and the responsiveness of the concurrent object to operations
(`latencies`). This comprehensive methodological approach is designed to
yield a lock mechanism that is both equitable and efficient.

= Previous Experience

#indent I have been engaged in programming for many years. Starting in the
interim between high school and college, I start contributing to an
open-source project, #link("https://github.com/Flow-Launcher/Flow.Launcher", [Flow Launcher]),
for several years. This project has since garnered over 5,000 stars on
GitHub. It is developed in C\#, a language that captivates me, especially
its asynchronous programming model and parallelism capabilities.

In particular, I am intrigued by the application framework we utilize, _WPF_,
which includes a feature known as Dispatcher to synchronize work to the UI
thread. This is akin to a delegation-styled lock. My focus has been on
enhancing the application's performance through the implementation of
various parallelism techniques.

My academic journey has included comprehensive coursework in systems
programming, specifically CS 537 and CS 564. Prior to embarking on the
proposed project, I have thoroughly studied two influencial texts in the
field of multiprocessor shared memory synchronization: Shared-Memory
Synchronization and The Art of Multiprocessor Programming, which served as
the foundational references for my work.

= Conclusion

#indent In conclusion, I propose to demonstrate that delegation-styled
locks suffers from the scheduler subversion problem. To remedy this
problem, I propose to integrate existing delegation-styled locks with "banning"
strategy to ensure their usage fairness. Further I propose to employ
stochastic methods to share the combining evenly.

For future improvements, the project plans to tackle the combiner's
response time issue by offloading non-critical work to a waiting thread
that is anticipated to experience a longer wait time. This could be
achieved through strategies inspired by the TCL Lock or by leveraging the
asynchronous programming model provided by many modern languages
(C++/Rust/C\#/JavaScript) and manage `Future` executions within a custom
runtime that adapts to lock-usage patterns.




#set par(leading: 0.5em)
#set block(above: 0em, below: 0.5em)
#bibliography("literature.yml", style: "acm-sig-proceedings.csl")