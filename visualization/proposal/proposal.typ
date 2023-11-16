#set par(leading: 1em)
#set block(above: 1.5em)

= Prelimilary

I would assume that we are familier the concept of asynchronous programming.

= Motivation

In the past few weeks I was thinking about what kind of system I may build keeping mind of the idea of usage-fair scheduling and delegation-style locking. Given that currently the major focus of fairness has kindly shift to the response time, I've thought whether we can do better for the combiner, by letting the waiters (that expected to wait long) to handle combiner's non-critical section, without sacrifying the benefit of combining.

That said, not only we want to wrap the critical section as some "task", we also want to wrap the non-critical section as some "task" as well. This seems to be cool idea at first. Given that the lock is "fair" or "usage-fair", the waiter will have a good estimation of how long it might take until their critical section. A simple example applying this information is the propotional backoff of a ticket lock. 

However, quickly I realize like we are having way too much waiter #footnote[Assuming the lock is highly contended] compared to the combiner that might be interested in serving the non-critical sections. However, what if the number of threads are less than the "critical job"? In other words, if the threads are publishing job and their decendent jobs that depends on the results, then we are having a more freely job scheduling system #footnote[Of course all this considering is pretty high level and assuming we are able to collect jobs from multiple threads into something organizable].

Then very quickly I realize that this becomes some form of threadpool with the consideration of lock with usage fairness, which might or might not be very interesting. Further, it is also hard to be used, producing something like a "callback" hell #footnote[I hear that when I learn about `promise` in _javascript_, but I learn _async/await_ even earlier.].

For example, I was thinking about how to turn the following code into something callable for both the critical part and non-critical part:

```py
def foo():
    while True:
        if cond:
            with lock:
                # do something
        else:
            # do something else
        
```

It is certainly possible, but likely not as easy as wrapping a code block as a closure like what we have done with the critical sections.

== Resolution 1

Once I realize that this is not much different from a "callback hell" issue in asynchronous programming, a natural idea is to look how people solve it in most of the modern programming language. From my mere knowledge, coroutine is the name of the solution and there are two type of it: 1) stackful coroutine and 2) _async/await_. I am only familier with the latter, so I will only talk about _async/await_.

Essentially we want to have something callable for the non-critical section. This doesn't have to be a closure, but rather a state machine, as how _async/await_ is generally implemented. Instead of passing a closure as a job, we pass something *resumable* such as a state machine. Thus, the job belongs to the combiner don't have to sacrifice the response time, while maintaining the benefits of combining.


== Resolution 2

Further being inspired by the work of transparent delegation to provide delegation-style lock in standard locking API @transparent_dlock_ref. There they have domonstrated the potential of capturing the critical section by capturing value of stack register (*RSP*) and the program counter (*RIP*). By switching to the captured stack and program counter, we can execute the critical section in the context of the waiter. 

#bibliography("literature.yml")