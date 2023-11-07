#import "../../shortcut.typ": *

== Experiment

A small experiment involving a bunch of threads incrementing a shared counter one per iteration is used to benchmark the response time for different locks. The experiment is run on the c220g2 in cloudlab, which has two Intel E5-2660 v3 10-core CPUs at 2.60 GHz (Haswell EP). @response_time_ecdf is the ECDF of the response time for each lock when running with 40 threads.


#figure(caption: [Response Time ECDF for 40 threads])[
    #image("../../../graphs/response_time_ecdf.svg")
]<response_time_ecdf>


The _x-axis_ is not linked across plot to allow seeing the individual patten of the ECDF for each lock. The duration of each critical section is super short, which also creates some unique patterns for the experiment.

=== Mutex/SpinLock/U-SCL

Mutex has the a very large range on the _x-axis_. Thus it is possible for a thread to wait very long even though all they wish to do is to provide a small increment to the shared counter.

SpinLock has a interesting pattern of the response time. The curve of the ECDF is very smooth, which is quite interesting. Other than that it has a similar behavior as Mutex.

U-SCL's response time behavior has been well analysis in its paper. The response time is very small for most scenerio given the presence of lock slice but it can be very large when waiting for a lock slice.


=== Delegation Locks

The main focus of this write up is about the discussion of response time of delegation locks, and specifically discussing the difference of response time when a thread is and is not combining. The yellow ecdf curve is the response time of delegation lock when a thread is combining, and the blue ecdf curve is the response time of delegation lock when a thread is not combining.

We can see the response of delegation locks are not skewed to the left as *Mutex/SpinLock/U-SCL*. This is likely because threads are not able to execute their critical section right after entering the locks (given that most delegation locks provides expected acquisition fairness). Though U-SCL also provides similar guarantee, the presence of lock slice changes it behavior. If we have a ticket lock, I would anticipate a similar behavior with it (*TODO*).

==== #fc

In the center of the graph, we can see the behavior of various variants of Flat Combining lock. Specifically, we see a pattern of _linear_ climbing when a thread is a combiner for the original #fc. This might indicate some _probability distribution_ of the number of threads that are waiting, since combiner of #fc traverse all threads node and try to execute their critical section. The blue line also extend to the right most as the yellow line, as the combiner may skip some thread when they are preparing to the job, which means they have to wait the next pass (or the thread lies at the end of the _linked list_).

The behavior of #fc-ban (*Flat Combining Fair* in the graph at the left bottom) has a similar distribution regardless whether a thread is combining, which is kindly interesting. Note that the _x-axis_ scale of blocking version is different from the one of spinning version (and also different from #fc). Theoredically, I anticipate to see similar result as #fc, but since the critical section is too short, the calculation of usage might get errored, causing some thread got banned unexpectedly. 

The _kink_ is interesting, but I cannot think of a reasonable explanation for it. This _kink_ also appears for #ccsynch-ban, so I believe might be caused by the huristic algorithm of banning.

==== #ccsynch

On the top of the graph, we can see that #ccsynch is providing a more concentrated distribution of response time, both combining or not.

==== #fc-skiplist

At the rightest of the second row, we can see the behavior of #fc-skiplist shows some expected pattern. The non-combining threads are having a seemingly short response time, and the combining threads are having a relatively large waiting time. The promise of #fc-skiplist is to provide a *CFS*-like scheduling policy, with a bound of volunteering time as combiner. Even though the combiner may experience long waiting time, it is bounded by the maximum servicing time of the lock.


