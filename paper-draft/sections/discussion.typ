= Discussion <sec:discussion>

== Blocking waiters <sec:absence>

Our FC-PQ prototype's waiters spin, which costs CPU time and limits it to no more threads than cores. Delegation does not require spinning: the combiner is independent of how waiters wait, and delegation locks can block their waiters until their requests complete @gupta2023tclocks. Blocking, however, introduces a cost that spinning avoids. A combiner can choose only among the requests pending when it decides, and a blocked thread that has just been served is still waking up when the combiner next decides, so its next request is not yet pending. The combiner must then either wait for the thread and leave the lock idle, or serve another thread and let the waking one fall behind. How a usage-fair delegation lock should make this choice, and whether a design can make its cost small, remains open.
