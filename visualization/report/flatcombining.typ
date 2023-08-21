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

#let flatcombining = [
  Flat Combining maintains a list of node that is owned by each thread. Each node contains the context of the thread and the job that the thread is trying to execute. The combiner will enumerate the list of nodes to check whether the node is ready. If the node is ready, the combiner will execute the job and update the node to indicate that the job is finished.

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

  The `pass` field is used to indicate the age of the lock, thus able to removing threads that hasn't executed job for too long.
  The `head` field is the head of the list of thread nodes.
  The `local_node` field is used to store the thread local node of the current thread, similar to `pthread_getspecific`.
  The `combiner_lock` is used for threads to elect the combiner. The implementation idea is retrieved from @rs_concurrent_ref (code are avaliable in @rawspinlock).


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

  The `age` field synchrnizes with the `pass` field of the lock to indicate the age of the node. If `age` is too far from `pass`, the node will be removed from the linkedlist and marked as false at `active`.
  The `f` field is the function pointer to the job that the thread is trying to execute. The `next` field is the pointer to the next node in the linkedlist. 
  The `parker` field is the parker that is used to block the thread when job is published but not yet executed (refer to @parker_impl).

  The original paper proposed that the ready state can be embedded into `f` field, thus saving a bool field @flatcombining_ref. However, we choose not to do this in our implementation for tow reason.

  The `combiner_time_stat` field is used to record the time that each thread becomes the combiner. It is not marked with `Atomic` because only the combiner (which is guarded by the `combiner_lock`) will overwrite this field (maybe volatile is needed?). 


  1. In rust, as we encapsulate the job into a trait object, whose pointer is a fat pointer (which contains two pointer). We don't want to have another layer of indirection that points to the fat pointer, which cannot be accessed atomically (with rust std). With an additional bool field as a write fence (`Release` Ordering), we make sure that the combiner will see the whole job before the node appears to be ready.
  2. We want to extract the waiting as a `parker` field, so that we can share blocking code among different locks. This is not so possible if we embed the ready state into `f` field.

  The overall algorithm can be roughly represented by the following graph:

  #illustration

  The insertion is done via a simple CAS loop similar to a singly linked list:

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

  The combiner will periodically remove the unactive nodes. The head of the list will not be removed to make sure the removal can be done when new node is added.
]