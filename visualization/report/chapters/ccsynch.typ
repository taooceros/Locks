#import "../shortcut.typ": *

#smallcaps[CC-Synch] maintains a first-in, first-out (FIFO)
queue of jobs. Unlike #fc, where each node corresponds to a
thread, in #smallcaps[CC-Synch]

each node signifies a job. Additionally, the selection of
the combiner is achieved without involving a lock; rather,
it's accomplished in a lock-free manner, relying on the
position of the job queue.

Intuitively, a thread will be elected as a combiner if one
of the following conditions is satisfied: 1. Its job is
positioned at the end of the queue. 2. Previous Combiner has
reached the execution limit.

In contrast to FlatCombining, it's important to note that an
execution limit is necessary to ensure that the combiner
does not execute an excessive number of critical sections.
The job queue has the potential to extend infinitely,
thereby causing the combiner to possibly prioritize jobs
from the queue indefinitely, potentially neglecting its own
task. In contrast, the maximum number of jobs performed by
FlatCombining's combiner is constrained by the number of
threads, which we can assume to be finite.

The algorithms can be outlined as follows:

+ Each thread maintains a mutable thread-local pointer to a
  node. Note that, unlike in FlatCombining where the node
  always belongs to the creating thread, in this scheme, nodes
  can be accessed and modified by different threads.
+ When a thread intends to execute a critical section:
  + It swaps its pointer storage with the current head node.
  + The task is then assigned to the old head node that was just
    swapped, and this node becomes connected to the previous
    node (which is now the new head).
+ To ensure lock-free operation, a somewhat intricate
  mechanism is employed:
  + Each node has two properties: `wait` and `completed`
  + Initially, `wait` and `completed` are set to false.
  + Before a thread swaps its thread-local node to head, it sets
    the `wait` property to true. The old-head then becomes the
    thread-local node of the thread.
  + Once the task assignment is done, the thread checks the `wait`

    property. If it'
    s true, the thread waits until it becomes false.
  + Once wait becomes false, the thread examines the completed
    property. If the task has not been completed, it assumes the
    role of the combiner.
  + The combiner will stop execution at `head` node, which doesn'

    t have children (there will always be an extra dummy node
    for swapping). Mark its wait as false, and exit.

H-Synch, although not currently implemented, represents a
NUMA-aware variant of #ccsynch
. The underlying concept is straightforward: it introduces
multiple queues based on NUMA nodes, and each combiner
competes for execution on the queue specific to its cluster.
This approach takes into account the NUMA architecture to
optimize task execution.
