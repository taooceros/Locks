#import "@preview/cetz:0.0.1"

#set heading(numbering: "1.1 ")
#set par(leading: 1.5em)
#set block(above: 2em)

#let todo = text(red)[*TODO!*]

#outline()

#pagebreak()

#let ccsynch = smallcaps([CC-Synch])
#let fc = smallcaps[Flat-Combining]

= Introduction

Delegation locking adopts the request-response style of communication to minimize shared data movement. Specifically, the waiter delegates their critical section to a combiner (#fc/#ccsynch) #cite("flatcombining_ref", "ccsynch_ref"), or a dedicated thread (RCL/ffwd) #cite("rcl_ref", "ffwd_ref"). We will describe the implementation details of these locks in @impl. Further, the idea of combining allows us to batch process operations for special data structure. For example, multiple insertion and deletion operations can be batched into a single operation for a linked list.

Noticeably, the idea of _lock slice_ utilized in U-SCL also accommodate with the idea of delegation-style locks by making sure that only one thread is executing the critical section at a given time slice, thus reducing the need of moving shared data around cores and improve performance @scl_ref. This is very similar to the goal of delegation-style locks, and thus provide similar performance boost. However, it has a serious drawback: within a valid _lock slice_, no other thread can hold the lock even when the previous thread is not holding the lock. Thus, the non-critical section needs to be very short or else the throughout will be greatly impacted. On the other hand, delegation-style locks provides the similar performance benefits without the drawback of _lock slice_. 

The main issue of delegation-style locks are the refactoring of old code, as it doesn't provide a similar API as regular locks. However, recent work has demonstrated the potential of transparent delegation to resolve this issue @transparent_dlock_ref.

On the other hand, most delegation-style locks inherently provides some fairness guarantee. There's no reason that the combiner shall treat their critical sections different from others', so most of the policy is to enumerate all ready jobs. For example, the combiner of FlatCombining (or the server of RCL) enumerates all the thread list to check whether a waiter is trying to execute a critical section. This is not the case for spin-lock. A few of the threads might dominate the lock usage and starve the others. If threads are repeatedly reacquired the lock, the thread that are releasing the lock will have some advantage to reacquire the lock as the cache synchronization is faster for them.

However, previous work has demonstrated that acquisition fairness of lock is not enough to mitigate the problem of scheduler subversion @scl_ref.  For example, @cc_loop_count has demonstrated the unfairness of #ccsynch even if it maintains a strictly FIFO order given varying critical section size. The 16 threads that are incrementing a shared counter are split into two groups: the first group will run for 10ns, and the second group will run for 30ns.
We can easily see that threads in the two groups contribute to the shared counter differently --- proportion to their critical section size.

#figure(caption: "#ccsynch Loop Count per thread")[
    #image("../graphs/ccsynch_loop_comparison_per_thread.svg")
]<cc_loop_count>

To mitigate these problems, we modify the original algorithm of FlatCombining and #ccsynch to achieve the fairness goal. We majorly adopted two strategies to mitigate the issue.

== Banning <ban_intro>

Similar to how U-SCL is implemented, we ban the thread that is trying to execute the critical sections for too long. Currently, we have implemented two fair variants of #fc and #ccsynch based on this idea.

The banning time is calculated as below:

$ max(0, "cs" times n_"thread" - "cs"_"avg") $

where the average critical sections are calculated incrementally.

$ "cs"_"avg" <- "cs"_"avg" + ("cs"-"cs"_"avg") / (n_"exec") $

The reason I use subtract an additional average critical section usage is to mitigate the cases where there's only a few threads that share similar critical section length. Without the subtraction, there might be some time when all threads are banned, which will greatly decrease the performance.

== Priority Based Structure

On the other hand, we can replace the linear concurrent data structure with a prioritized data structure. For example, Flat Combining maintains a Linear list of threads. We can replace that with a Concurrent Skip-List, which allows us to prioritize the threads based on their lock usage.


= Implementation Details <impl>

This work implement the following delegation-style locks in _rust_ (so we assume some understanding toward _rust_): FlatCombining @flatcombining_ref, #ccsynch @ccsynch_ref, RCL @rcl_ref. 

== Job Type

Each lock implement a common trait (interface) as follows, where the generic type represents the shared data:

```rs
pub trait DLock<T> {
    fn lock<'a>(&self, f: impl DLockDelegate<T> + 'a);

    #[cfg(feature = "combiner_stat")]
    fn get_current_thread_combining_time(&self) -> Option<std::num::NonZeroI64>;
}
```

and the trait `DLockDelegate` refers to the critical sections defined as follows:

```rs
pub trait DLockDelegate<T>: Send + Sync {
    fn apply(&mut self, data: DLockGuard<T>);
}
```


The `DLockGuard<T>` is similar to the `MutexGuard<T>` in _rust_, which implements the `Deref` and `DerefMut` traits. The `DLockGuard` is used to access the shared data. Unlike `MutexGuard`, which is also used to release the lock when dropped, the lock is automatically released when the delegate finishes.

```rs
pub struct DLockGuard<'a, T: ?Sized> {
    data: &'a SyncUnsafeCell<T>, // UnsafeCell is used to allow interior mutability
}
```


To make the lock easy to use, we implement `DLockDelegate<T>` for lambda function type `FnMut(DLockGuard<T>) + Send + Sync`. Thus, the user can use the lock as follows:

```rs
lock_type.lock(|mut guard: DLockGuard<u64>| {
    ...
});
```

== Parker <parker_impl>

#todo

== Delegation-style Lock


To implement a delegation-style lock, we need to consider to aspect.
1. How the jobs are published
2. How the combiner is selected

There are two natural ways to publish jobs: per thread or per job. 

For "per thread", I mean each thread owns a dedicated memory location (node) that allows it to publish a job with its context. Then the combiner enumerate the threads' nodes, check whether the node is ready, and if it is ready, execute the job. Flat Combining (@flatcombining), RCL (@rcl) and #smallcaps[ffwd] adapts this way of publishing jobs.

For "per job", each thread should publish the job for execution to a dedicated job queue. The combiner will then enumerate the job queues to execute the jobs. #smallcaps[CC-Synch] and its variants adapt this way of publishing jobs.


=== Flat Combining <flatcombining>

#import "flatcombining.typ": flatcombining

#flatcombining

=== Remote Core Locking <rcl>

Similar to FlatCombining, RCL maintains a list of node that is owned by each thread. The main difference is that RCL dedicate a thread to behave as the server (combiner). Therefore, the original paper suggests that it behaves like a remote procedure call. Subtly, rcl employs an array of nodes, where FlatCombining maintains a linked list of nodes. This should provide better performance if the number of inactive threads are small.

One of the main goal of RCL is that it allows easy migration of legacy code. Therefore, it employs complicated algorithm to make sure that the server thread won't be blocked or actively spinning so that the performance is affected. Further, the design of RCL allows the server to handle multiple lock so that legacy code can be easily ported. We didn't implement the full version of RCL and thus will not focus on these.

Though, one of the main goal is kept in our implementation that one server can handle different types of locks. This is a large burden toward _rust_ type system, as we want to statically dispatch the type information so that no performance regression will be found. The detail of the implementation is in appendix @rcl_detail_impl.



=== #smallcaps[CC-Synch/H-Synch]

#smallcaps[CC-Synch] maintains a FIFO queue of jobs. In contrast to #fc, each node represents a job instead of a thread. Further, the combiner is not elected by a lock, but instead done lock-freely based on the position of the job queue.

Intuitively, a thread will be elected as a combiner if one of the following conditions is satisfied:
1. Its job is at the end of the queue
2. Previous Combiner has executed too much critical sections.

Compared to FlatCombining, it is important to note that an additional limit is required to make sure that the combiner will not execute too many critical sections. The job queue can be extended infinitely, and thus the combiner might never return to complete its other jobs. In the contrast, the largest number of jobs done by FlatCombining's combiner is bounded by the number of threads, which we can assume to be finite.

The algorithms can be sketched below

+ Each thread holds a _mutable_ thread-local pointer to a node. (Note that in FlatCombining the node always belong to the thread that creates it).
+ When a thread wants to execute a critical section, it swaps the pointer storage with the current head.
+ Then assign the task to the *old head node* that is just being swapped, and connect to the previous node that is now the *head*.
+ Then there's a slightly complicated mechanism to make sure no lock is required.
  + Each node has two properties: `wait` and `completed`
  + `wait` will be false if no one is combiner. Before a thread swaps its thread-local node to head, the `wait` will be assigned to true.
  + After a thread assigns its task. It will check the `wait` property. If `true`, then wait till it becomes false.
  + After `wait` becomes `false`, it will check the `completed` property. If it hasn't been completed, then the thread becomes the combiner. There's a subtle argument about ordering that I will skip saying why this is enough to only check the `completed` property after `wait` is false.
  + The combiner will stop at the `head` node, where it doesn't have children (there will always be an extra dummy node for swapping). Mark its wait as false, and exit.

H-Synch is a NUMA-awareness version of #ccsynch, which is not yet implemented.

== Fair delegation-style locks

We present two kinds of approach to implement a fair delegation-style locks. One of the idea is based on banning (similar to how SCLs is implemented @scl_ref), and the other is based on priority-based structure.

=== Banning

The idea of banning is to ban the thread that is trying to execute the critical section for too long. Banning is not special or easy to implement for delegation-style locks, but given that it is the most natural way of implementing a fair lock, we still implement a fair variant of FlatCombining and #ccsynch based on this idea.


#let fc-ban = [#fc (Banning)]
==== #fc-ban <flatcombining_banning>

Adapting the idea of banning, the combiner will calculate how long it takes to execute the critical sections, and calculate when the thread should be banned based on the algorithm in @ban_intro. Similar to how U-SCL is implemented, only the timestamp that the thread can resume execution will be recorded.

When enumerating the thread list, the combiner will firstly check whether the thread is banned, and only then perform the ordinary FlatCombining algorithm.

Note that this will involve some additional cost, as more nodes will behave "inactive" because they are banned. One resolution is to remove threads that are banned from the list, but this will involve some additional cost to maintain the list. Some heuristic algorithm may be used to decide whether the node should be removed.

Another subtle thing is that we can let the waiter decide whether it is banned instead of wasting combiner's time to check whether the thread is banned. This is the way #ccsynch Ban is implemented @ccsynch_banning, but involves some additional cost of transferring the critical section lengths of the job. Given we implement a pre-wake mechanism, one resolution is to let waiter record the timestamp when it is pre-waked, and calculate the critical section when it is eventually waked.


#let ccsynch-ban = [#ccsynch (Banning)]
==== #ccsynch-ban <ccsynch_banning>

In contrary to #fc-ban, the combiner now have no idea about whether a thread is banned. Instead, each waiter will stop inserting its node into the job queue when it is banned. The combiner will only record the length of critical section and send that to the nodes that are waiting. The node will then decide when it is banned until based on the algorithm in @ban_intro.

Noticeably, the banning strategy utilized here involves a much less performance penalty compared to #fc-ban given that combiner won't need to traverse useless nodes. However, this also suggests that slightly threads may be needed to achieve the same number of combined jobs as #ccsynch.

=== Priority-based Structure

The idea of priority-based structure is to replace the linear data structure with a prioritized data structure. For example, FlatCombining maintains a Linear list of threads. We can replace that with a Concurrent Priority Queue, which allows us to prioritize the threads based on their lock usage. This idea is similar to _Linux CFS_, which uses a red-black tree to prioritize the threads based on their execution time. This method is more inherent to delegation-style locks, as there is a scheduler like character: the combiner.

#let fc-pq = [#fc (Priority-Queue)]
==== #fc-pq (Not implemented)

We can replace the linked-list of FlatCombining with a priority queue (either a heap or a balanced tree). However, they are not inherently concurrent as linked list, and thus either we need to find a concurrent implementation or make sure mutual exclusion to them. Luckily, the combiner is a natural character to modify and control the queue.

Similar to #fc, we still utilize a thread-local node to publish the job. The difference is that thread will push the node to a prepared queue (linked list) lock-freely, and then the combiner will pop the node from the queue and push the thread node to the priority queue. 

Before each pass, the combiner will check whether there are additional prepared thread waiting to be inserted into the priority queue. If there is, then the combiner will pop it from the prepared queue and insert into the priority queue.

Then it just executes jobs like #fc through the prioritized data structure.

#let fc-skiplist = [#fc (Skip-List)]

==== #fc-skiplist <flatcombining_skiplist>

We can maintain a job queue that is prioritized based on the critical section length with the help of a concurrent skip list. The combiner will be elected as #fc and then execute the job from the head of the priority queue.

This is probably one of the simplest idea one can think of when trying to adapt the idea of priority-based structure to implement a fair delegation-style lock. Subtly, it is not accurate to call it a variant of #fc, as it opts to the idea of a job queue (similar to #ccsynch) instead of a thread queue.

Similar to #ccsynch, a combiner limit is important here, given that it is possible to have infinitely many jobs waiting to be executed. Our current implementation is based on a _contribution_ limit, which is similar to the _H_ limit in #ccsynch but calculated based on timestamp instead of number of jobs.

Currently, we use a 3rd-party implemented concurrent skip list, which is not yet optimized for our use case @crossbeam_skiplist_ref.

===== Future work

The election process can be done similar to how #ccsynch is done to avoid election through a lock. This will require a customized concurrent skip-list and a more involved understanding toward how to elect a combiner.

=== Response Time

Beyond lock-usage unfairness, delegation-style lock involves an additional unfairness. The combiner is doing work for others, and thus the response time of the combiner is greatly impacted. For example, in #fc, the combiner of the lock will need to wait the worse case of the response time, which is twice as the average response time of the waiters. This is a serious issue for #fc when the waiter are blocked to wait. As shown in the left plot of @combining_time_box_plot, two of the 32 threads are combining more than 80% of the time. The situation will become even worse when thread number if larger. (Remark: When implementing the first version of #fc in _rust_, I found that the blocking version of #fc is faster than the spin wait one, and the analysis of combining time resolves the reason behind that.)

#figure(caption: "Combining Time Distribution of 32 threads execution")[
    #image("../graphs/combining_time_box_plot.svg")
]<combining_time_box_plot>

This gives us some insight that why we not only need to care about the scheduling of waiters, but also the scheduling of the combiner. From another aspect, we can reduce the average response time by carefully choosing which thread should become the combiner. For example, we should always let the tail of the threads list to become the combiner in #fc. Because of the "flat" execution, the tail node will always be executed at last, which means that its response time is always the sum of all critical sections in the current pass. Therefore, becoming a combiner will not give any penalty toward its response time. In the contrary, if the head node becomes the combiner, its response time reduces greatly from its own critical section length to the sum of all critical sections in the current pass (this is not always the case though. Considering the case that its job is ready after combiner already started and skipped its node).

#text(green.darken(30%))[Experiments regarding response hasn't been added, and all these ideas also haven't been implemented.]

==== Combiner Slice

The idea of _Combiner slice_ is similar to the idea of _lock slice_, but slightly differently. A thread should always become the combiner given a time slice, and yield when other combiner slice is valid. This might yield performance boost when the length of the slice is carefully chosen, but also face the same issue of _lock slice_ that the non-critical section needs to be short enough.

#todo


#let fc-ban-combiner-slice = [#fc (Banning, Combiner-Slice)]

==== #fc-ban-combiner-slice

#text(blue)[
    #todo
    This is not a good implementation and is not an implementation of the combiner slice I describe above.
]

= Experiments

We benchmark the performance by incrementing a shared counter for a given time slice.

The following locks are benchmarked in our example:
+ #fc
+ #fc-ban
+ #fc-ban-combiner-slice (#text(blue)[Please ignore now])
+ #fc-skiplist
+ #ccsynch
+ #ccsynch-ban
+ RCL
+ Mutex (rust)
+ Spinlock (@rawspinlock)
+ U-SCL (@scl_ref)

The workers will be split into two groups. The first group will run for 10ns, and the second group will run for 30ns.

Since all delegation-style is implemented with a generic parker. We will split the result based on the two parker.

== General Performance Comparison

#figure(caption: "Performance Comparison")[
    #image("../graphs/loop_comparison_together.svg")
]<loop_comparison>



#bibliography("literature.yml")

#pagebreak()

= Appendix

== RawSpinLock <rawspinlock>

The RawSpinLock is implemented as a test-and-test-and-set lock with exponential backoff:

```rs
#[derive(Debug)]
pub struct RawSpinLock {
    flag: AtomicBool,
}

unsafe impl RawSimpleLock for RawSpinLock {
    fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
        }
    }

    #[inline]
    fn try_lock(&self) -> bool {
        if !self.flag.load_consume() {
            self.flag
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        } else {
            false
        }
    }

    #[inline]
    fn lock(&self) {
        let backoff = Backoff::new();

        while !self.try_lock() {
            backoff.snooze();
        }
    }

    #[inline]
    fn unlock(&self) {
        self.flag.store(false, Ordering::Release);
    }
}
```

=== Remark

The additional load doesn't seem to provide better performance to FlatCombining given that most of the time the thread will only `try_lock` once and then block for a while. However, it does provide better performance when used completely alone.

== RCL Implementation Details <rcl_detail_impl>

#todo