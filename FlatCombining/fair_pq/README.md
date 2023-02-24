# Fair Flat Combining (Priority Queue Based)

We follow the standard Flat Combining Strategy of Combining, but picking jobs with a priority queue.

## Idea

There are two ways of handling priority queue. One way is to use a lock-free priority queue, and the other way is to use the lockfree linkedlist to let threads register nodes, and let the combiner insert the node for other clients.

## Algorithms (tentative)

1. Each thread retrieves their own specific node, if it is active, 