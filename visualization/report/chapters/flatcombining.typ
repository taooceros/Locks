#import "@preview/cetz:0.0.1"

#let illustration = cetz.canvas({
  import cetz.draw: *


  let square(x,y,r) = rect.with((x,y), (x+r,y+r))

  
  circle((0,0), radius: 1.5, name: "check_active")
  circle((), radius: 2)
  
  content((rel: (0,0)))[
    Check whether 
    
    Node is Active
  ]
  
  circle((rel: (5,0)), radius: 1.5 ,name: "try_combine")
  
  content(())[
    try to combine
    #set align(center)
    (trylock)
  ]
  

  circle((rel: (5,3)), radius: 1.5, name: "combine")

  content(())[
    #set align(center)
    
    Traverse the list\
    and execute
  ]

  circle((rel: (3,-6)), radius: 1.5 ,name: "wait")

  content(())[
    #set align(center)
    Wait for job\
    finish
  ]
  
  line("try_combine.right", "combine.left", mark: (end: ">"), name: "active_yes")

  content("active_yes")[
    Yes
  ]

  line("try_combine.right", "wait.left", mark: (end: ">"), name: "active_yes")

  content(())[
    
  ]

  line("check_active.right", "try_combine.left", mark: (end: ">"), name: "check_active_yes")

  content("check_active_yes")[
    #linebreak()
    yes
  ]

  
  circle((0,-5), radius: 1.5, name: "insert_node")
  content(())[
    Insert Node
  ]

  line("check_active.bottom", "insert_node.top", mark: (end: ">"), name: "check_active_no", stroke: (dash: "dotted"))

  content("check_active_no")[
    no
  ]

  line("insert_node.right", "try_combine.bottom", mark: (end: ">"), name: "check_active_no")

  place-marks(bezier("wait.right", "check_active.bottom-right", (15, -5), (5, -10), stroke: (dash: "dotted")), (mark: ">", pos: 1))
  

  content((5,-5))[
    periodic
  ]

  
  line("combine.right", "wait.top", mark: (end: ">"))
});

#let fc = smallcaps[Flat-Combining]

#fc upholds a list of nodes owned by each thread. Each node incorporates the thread's context and the job it intends to execute. The combiner iterates through this list of nodes, evaluating their readiness. If a node is deemed ready, the combiner proceeds to execute the job and subsequently updates the node to signal the job's completion.

The struct is defined as follows:

```rs
#[derive(Debug)]
pub struct FcLock<T, L, P>
where
    L: RawSimpleLock,
    P: Parker,
{
    pass: AtomicU32,
    combiner_lock: CachePadded<L>,
    data: SyncUnsafeCell<T>,
    head: AtomicPtr<Node<T, P>>,
    local_node: ThreadLocal<SyncUnsafeCell<Node<T, P>>>,
}
```

- The `pass` field serves to indicate the age of the lock, making it possible to remove threads that have not executed a job for an extended period.
- The `head` field is the head of the list of thread nodes.
- The `local_node` field is designed to store the current thread's local node, akin to `pthread_getspecific`.
- The `combiner_lock` is utilized for thread-based combiner election. The implementation idea is retrieved from @rs_concurrent_ref (code are avaliable in @rawspinlock).


The node struct is defined as follows:

```rs
pub(super) struct Node<T, P>
where
    P: Parker,
{
    pub(super) age: u32,
    pub(super) active: AtomicBool,
    pub(super) f: CachePadded<Option<*mut (dyn DLockDelegate<T>)>>,
    pub(super) next: *mut Node<T, P>,
    pub(super) parker: P, // id: i32,
    #[cfg(feature = "combiner_stat")]
    pub(super) combiner_time_stat: i64,
}
```

- The `age` field synchronizes with the `pass` field of the lock, indicating the age of the node. If the `age` is significantly different from `pass`, the node will be removed from the linked list and marked as inactive (false in the `active` field).
- The `f` field represents the critical section.
- The `next` field is the pointer to the next node in the linkedlist. 
- The `parker` field is the parker that is used to block the thread when job is published but not yet executed (refer to @parker_impl).
- The `combiner_time_stat` field is used to record the time that each thread becomes the combiner. It is not marked with `Atomic` because only the combiner (which is guarded by the `combiner_lock`) will overwrite this field (maybe volatile is needed?). 

The original paper proposed that the ready state can be embedded into `f` field, thus saving a boolean field @flatcombining_ref. However, we choose not to do this in our implementation for two reason.

+ In _rust_, we encapsulate the job into a trait object. A pointer pointing to a trait object is a fat pointer (which contains two pointer). Atomic operation toward such double-sized field are not available universally. Introducing another layer of indirection that points to the fat pointer to make it accessible atomically will involve additional cost. With an additional bool field used as a write-fence (`Release` Ordering), we ensure that the combiner views the entire job before the node is perceived as ready.
+ We aim to extract the waiting process into a `parker` field, enabling the sharing of blocking code across different locks. However, embedding the ready state into the `f` field would hinder this capability.

The overall algorithm can be roughly represented by the following graph:

#illustration

The insertion is done via a simple CAS loop as a singly linked list:

```rs
fn push_node(&self, node: &mut Node<T, P>) {
    let mut head = self.head.load(Ordering::Acquire);
    loop {
        node.next = head;
        match self
            .head
            .compare_exchange_weak(head, node, Ordering::Release, Ordering::Acquire)
        {
            Ok(_) => {
                node.active.store(true, Ordering::Release);
                break;
            }
            Err(x) => head = x,
        }
    }
}
```

The combiner will regularly remove inactive nodes. Subtly, the head of the list won't be removed to ensure that removal can be safely performed even when new nodes are being added.
