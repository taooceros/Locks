# Fair Flat Combining (Priority Queue Based)

We follow the standard Flat Combining Strategy of Combining, but picking jobs with a priority queue.

## Idea

There are two ways of handling priority queue. One way is to use a lock-free priority queue, and the other way is to use the lockfree linkedlist to let threads register nodes, and let the combiner insert the node for other clients.

## Algorithms (tentative)

1. We do the node insertion/deletion same as FlatCombining
2. As a combiner, the thread iterate over all the active nodes, and add them into the priority queue.
3. Pick jobs from the priority queue (sounds silly right).

## Thoughts

- The priority queue here doesn't do anything more than soring the jobs...
  - If we can assume that each threads acquiring the lock is busy enough, maybe we can decide to not pop the node out of the priority queue, but only peek and change priority...(this might requires a good algorithm to do it)
   - Draft One: If the top node doesn't have job available yet, change its prority by decreasing it by average execution time...? Or we can measure the average time between two critical section...
 - Another idea is to plainly use a lock free priority queue (either skiplist or array list)...but I am wondering whether the synchronization cost is high enough such that letting one thread controlling the priority queue might be cheaper...
 
## Future TODO
 
 - [ ] Implements Lock Free priority queue and compare their performance
 - [ ] Implements the priority queue that doesn't pop out nodes but instead do a panelty update to node at front that doesn't have job.


## Additioanl Idea

- To take the same idea of adding nodes once per life as flat combining (somehow it is actually being removed even in flat combining), we add the nodes to a linkedlist, and take the advantage of combiner to let the combiner adding them into a priority queue. Then let the combiner to combine the tasks, and every time it executes one new jobs, it checks whether there're new threads waiting to be added to the priority queue (maybe we can just do this once per combiner).
- Another idea is to simply Implements a lock-free priority queue, and let threads add themselves into the priority queue (which is suggested above).