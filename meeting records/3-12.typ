== TODO

+ Summary of what's being done (experiment data) and some plan
+ Assumption about thread is continuously acquirng the lock
+ Summer
+ Next Spring

== Stochastic Backoff of Combiner (Flat Combining)

Backoff by average critical section size

#let cs = "cs"

$
  PP("Backoff") = min(1, cs slash cs_"avg")
$

or maybe with some special function like sigmoid

$
  sigma (x) = 1/(1+e^(-x))\

  PP("Backoff") = min(1, sigma (cs slash cs_"avg"))
$

maybe also some adjustment based on the number of times of combining?

== Relaxed Priority Queue

Something like SparyList @spraylist_ref.

== Plain Publish with a Balanced Tree for Combiner

Assuming every thread is continuely acquiring the lock (and not quiting), we can use a fixed size array to store the publish node. Then the combiner just use arbitrary scheduling policy to access the node. If the node hasn't published job, we do exponential backoff for the node?

#bibliography("../reference/literature.yml", style: "../reference/acm-sig-proceedings.csl")


== Weekly Plan

- Revise benchmark
- Redone experiment
- Write a experiment report