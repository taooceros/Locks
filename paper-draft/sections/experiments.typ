= Proposed Experiments <sec:experiments>

The experiments below test the claim that coupling who is served with where the critical section runs is what makes usage fairness expensive, and that delegation removes this cost. Each states a prediction that could fail.

== Hypotheses <sec:hypotheses>

- *H1.* Locks whose served thread runs its own critical section, such as the FIFO MCS queue lock and CFL, lose throughput as the data a critical section touches grows, at fixed critical-section length and fixed fairness. NUMA-aware variants that keep the lock on one socket recover throughput only by giving up usage fairness.
- *H2.* u-SCL stays fair by reserving the lock for its slice owner, which shows up as time the lock sits idle while other threads wait.
- *H3.* FC and FC-PQ throughput stays flat as the data grows, because the combiner keeps it in one cache, while FC-PQ's Jain index over per-thread critical-section time stays at or above 0.95 when half the threads issue critical sections eight times longer than the rest.
- *H4.* The data size at which delegation overtakes the best non-delegating lock shrinks as the fairness target tightens.

== Working-set sweep <sec:exp-working-set>

This is the core experiment. A synthetic critical section touches $W$ cache lines, with $W in {1, 4, 16, 64, 256, 1024, 4096}$, and performs a fixed amount of computation. We run 8, 16, and 32 threads, one per physical core, both within one socket and across sockets; half the threads issue critical sections eight times longer than the others. We compare MCS, a ticket lock, CLH, CFL, u-SCL, FC, and FC-PQ.

For each lock we report throughput, the Jain index over per-thread critical-section time, the time the lock is idle while a thread waits, and per-operation cache misses and cross-core transfers from hardware counters. For FC and FC-PQ we also record how many requests the combiner serves per pass and how often it changes threads, the inputs of our analysis framework.

We expect the non-delegating locks to slow down as $W$ grows while FC and FC-PQ stay flat, giving a crossover $W^*$ beyond which FC-PQ is faster. At small $W$, delegation's per-request overhead exceeds the migration it saves, and we expect no delegation lock to win there. The result counts as support only if $W^*$ appears in both placements, with ranges that do not overlap across at least five trials.

== Fairness-granularity sweep <sec:exp-granularity>

The second experiment varies how tightly each lock enforces fairness: the slice length of u-SCL and CFL, and the selection window of FC-PQ. For each setting we plot throughput against the largest gap in critical-section time between any two threads. Locks that hand data from core to core trace a curve on which tighter fairness costs throughput; we expect FC-PQ to lie off that curve.

== Database confirmation <sec:exp-databases>

The third experiment checks that the ordering from the working-set sweep holds in real systems. In UpScaleDB, an embedded key-value store protected by a single global lock, we vary the number of preloaded records (1 thousand versus 1 million) as the working-set knob and collect the same hardware counters. In redb, an embedded transactional key-value store with a single-writer lock, we run writers that insert 1 and 64 records per transaction. Both runs use waiters that sleep rather than spin, so that threads can outnumber cores.

== Fidelity to SCL <sec:exp-scl>

If the first three experiments support the claim, we repeat Patel et al.'s UpScaleDB setup @patel2020scl: a disk-backed database with `fsync`, four find threads and four insert threads on four CPUs for 120 s, measuring fairness the way SCL does, over each thread's opportunity to use the lock. This shows whether FC-PQ fixes the scheduler subversion SCL reported in the workload where it was first observed.
