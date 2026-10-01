= Discussion <sec:discussion>

== Blocking waiters <sec:absence>

Our FC-PQ prototype's waiters spin, which costs CPU time and limits it to no more threads than cores. Waiters could instead block, but blocking makes fairness harder. Suppose threads A and B both submit requests continuously. If A blocks after each request, it takes several microseconds to wake up and submit its next one. During that time the combiner sees only B's requests. It can serve B, and A receives less than its share; or it can wait for A, and the lock sits idle. Spinning waiters avoid this choice because they submit their next request immediately. How a usage-fair delegation lock should handle blocked waiters remains open.
