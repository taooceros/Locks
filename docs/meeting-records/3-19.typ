= Experiment

== Locks

We have tested the following locks:

+ Mutex (Rust stdlib)
+ SpinLock (Test and test and set)
+ Flat Combining (FC) @flatcombining_ref
+ Flat Combining with Banning
+ CCSynch @ccsynch_ref
+ CCSynch with Banning
+ DSMSynch @ccsynch_ref
+ U-SCL


== Synthetic Workload

=== Goal

Measuring the throughput of different locks under different contention levels.

=== Design

Each thread will try to acquire the lock, apply the critical section, release the lock, and apply non-critical section, and then repeat the process.

==== Critical Section

We have two different critical sections length to demonstrate the impact toward fairness when imbalance workload is presenting.

The threads will be groupped into two groups.

+ Group 1: 1000 iterations
+ Group 2: 3000 iterations

==== Non-Critical Section

We have performed the experiment for different length of non-critical section to demonstrate the throughput under different contention levels.

+ Zero (highly contended): The threads will try to acquire the lock as soon as it is released (as long as it finish some statistics).
+ 10 iterations
+ 100 iterations
+ 1000 iterations
+ 10000 iterations
+ 100000 iterations

== Fetch and Multiply

=== Goal

To measure the throughput of different locks under a very short critical section with no native atomic operation.

=== Design

Each thread will try to acquire the lock, fetch the value, multiply it by 1.000001, and release the lock.

=== Lock-Free Option

We implement a lock-free version of the algorithm using CAS.

=== Non-Critical

We will have a short non-critical section that do an empty loop for a random number of iterations (1-8). This is similar to how the experiment is implemented in the paper presenting #raw("CCsynch") and #raw("DSM-Synch") @ccsynch_ref.

== Result



#bibliography("../reference/literature.yml", style: "../reference/acm-sig-proceedings.csl")