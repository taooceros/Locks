+ Delegation Lock Introduction
  + Advantages
  + Problems
+ Problem
  + Usage Fairness
  + Scheduling Subversion
+ Solution
  + Banning
    + Advantage
      + Easy to implement
    + Disadvantage
      + May create gap that prevents all threads from getting the lock
  + Priority Queue
    + Advantage
      + No gap
      + Fair
      + Single-threaded implementation is fast
    + Challenges
      + Multi-threaded lock-free implementation is complex and slow
      + Hard to design protocol for threads to publish to pririty queue
  + Other Scheduling Mechanism
+ Protocol Design
  + Idea
    + Thread publish job to combiner in non-scheduled state
    + Combiner schedule the job in order
  + Job publish
    + A MPSC channel
  + Responsibility
    + Combiner Election
      + A single `AtomicBool`
    + Combiner
      + Maintain a priority queue
      + Poll a channel to get new node from submitter
      + de-activate node (when?)
    + Waiter
      + Publish a node to a channel (MPSC channel)
  + Challenges
    + Publishing node can be expensive
      + May cache a node belongs to the thread
    + When do combiner check the channel
    + When a thread is about to enter sleep state
+ Delegation Styled Lock Job Post Type
  + Thread Level Queue
    + Flat Combining
    + node $->$ thread
    + Advantage
      + Simple
      + Fast
    + Challenges
      + thread sparsely enter the queue, it may starve
  + Job Level Queue
    + CCSynch
    + node $->$ job
    + Advantage
      + no need to iterate over thread that hasn't published job
    + Challenges
      + More complex
      + Hard to design protocol for threads to publish job
