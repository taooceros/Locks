# Flat Combining

The algorithm is described by [Flat Combining and the Synchronization-Parallelism
Tradeoff](https://people.csail.mit.edu/shanir/publications/Flat%20Combining%20SPAA%2010.pdf)

The idea is simple. Each threads register a node in the lock, and delegate their work to the node.
Whenever a thread want to execute its critical section, it will try to acquire a lock inside the FC-Lock.
If it succeed, becomes a combiner to execute jobs that has been registered in the FC-Lock.

## Algorithm

1. Retrieve thread specific node for the lock, and put everything needed to execute the critical section into the node.
2. Make sure that the node is active:
   - If not active insert the node to the head of list of nodes through CAS.
3. Try to acquire the lock to become the combiner (a test-test-and-set try lock)
   1. If succeed, iterate over the nodes: If their `delegate` field is not `NULL`, execute it with the `context` field and set `delegate` field to be `NULL`, and set the result of the critical section to `context` field. (Note it might have some race issue that `delegate` is set but `context` is not, which might create issue. Therefore, we will need a memory fence after setting the delegate field)
   2. If not succeed, just wait, and periodically wake up and try to become the combiner again (to encounter situation where a thread inserted node after a thread is executing other threads' critical sections.)
4. If the `delegate` field is `NULL`, just return the result in the `context` field.