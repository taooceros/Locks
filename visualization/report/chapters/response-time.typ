#import "../shortcut.typ": *

Delegation-style locks introduce an additional layer of
unfairness beyond the conventional concern of unfairness
arising from lock usage. The combiner, responsible for
executing tasks on behalf of others, becomes subject to a
significant impact on its own response time. To illustrate,
consider the case of #fc
, where the combiner for a lock must endure a wait time that
can be twice the average response time of the threads it's
assisting. This situation becomes particularly problematic
in scenarios where the waiters are blocked, as evidenced in
the left plot of the @combining_time_box_plot. In this
specific example, two out of the 32 threads are engaging in
combining for over 80% of the time. This issue escalates
further as the thread count increases. (Note: When
developing the initial version of #fc
in Rust, it was observed that the blocking variant of #fc

outperformed the spin-wait version, and an analysis of
combining time offered insights into this phenomenon.) #ccsynch

also suffers from this issue, which is only noticable with
32 threads.

#figure(
  caption: [
    Combining Time Distribution of 32 threads execution (0ns
    critical section)
  ],
)[
    #image("../../graphs/combining_time_box_plot.svg")
  ]<combining_time_box_plot>

This insight underscores the significance of not only
optimizing the scheduling of waiters but also addressing the
scheduling of the combiner itself. Furthermore, it suggests
that we can mitigate average response times by thoughtfully
selecting which thread assumes the role of the combiner. A
potential is the deliberate choice to designate the tail of
the threads list as the combiner in #fc.

Due to the"flat" execution nature, the tail node is
consistently executed last, implying that its response time
equates to the cumulative sum of all critical sections
within the current pass. Consequently, adopting the role of
a combiner does not impose any penalty on its response time
(if there's no combiner executing). Conversely, if the head
node were to become the combiner, its response time would
experience a significant regression, shifting from its own
critical section length to the summation of all critical
sections within the current pass.


#include "response-time/experiment.typ"