= Discussion <sec:discussion>

== Absent threads <sec:absence>

Delegation does not remove every cost of fairness; one remains that is independent of data placement. A lock can choose only among the threads that are waiting when it decides, yet a thread may want the lock without waiting for it, because it is still waking up or has been preempted. The lock must pay for this _absent_ thread in one of three ways: it can hold the lock for the thread and leave the lock idle, give the turn to a waiting thread and let the absent one fall behind, or have waiters spin so that none is ever absent and pay in CPU time. Our FC-PQ prototype pays in CPU time. Its combiner is independent of how waiters wait, so it can in principle be paired with either of the other two, but which choice serves a usage-fair delegation lock best, and whether a design can make the cost small, remains open.
