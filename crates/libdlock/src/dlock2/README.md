# dlock2/ — DLock2: Function-Delegate Delegation Locks

Primary API generation. User passes input data `I` through `lock(data: I) -> I`; the combiner applies a delegate function `Fn(&mut T, I) -> I`.

## Trait

```rust
pub trait DLock2Delegate<T, I>: Fn(&mut T, I) -> I + Send + Sync {}

pub unsafe trait DLock2<I>: Send + Sync {
    fn lock(&self, data: I) -> I;
    fn get_combine_time(&self) -> Option<u64>;  // with combiner_stat feature
}
```

The `unsafe` marker reflects that implementations use internal unsafe operations (SyncUnsafeCell, atomic orderings).

## Enum Dispatch

`DLock2Impl<T, I, F>` uses `#[enum_dispatch]` for zero-cost dispatch over all variants:

```
FC | FCBan | CC | CCBan | DSM | FC_SL | FC_PQ_BTree | FC_PQ_BHeap |
SpinLock | MCS | Mutex | USCL | C_FC | C_CC
```

## Variants

### Delegation Locks (combiner-based)

| Module | Type | Fairness Strategy |
|--------|------|-------------------|
| `fc/` | `FC<T, I, F, L>` | None — combiner serves all pending nodes |
| `fc_ban/` | `FCBan<T, I, F, L>` | TSC-based banning: threads exceeding fair share are temporarily skipped |
| `cc/` | `CCSynch<T, I, F>` | FIFO linked-list job queue |
| `cc_ban/` | `CCBan<T, I, F>` | FIFO + banning |
| `dsm/` | `DSMSynch<T, I, F>` | Double-buffer swap combining (two alternating request buffers) |
| `fc_sl/` | `FCSL<T, I, F, L>` | Skip-list ordered by cumulative usage |
| `fc_pq/` | `FCPQ<T, I, PQ, F, L>` | **Priority queue by cumulative usage — key algorithm** |

### Non-Delegation Baselines

| Module | Type | Description |
|--------|------|-------------|
| `spinlock.rs` | `DLock2Wrapper<T, I, F, L>` | Generic wrapper for any `lock_api::RawMutex` |
| `mcs/` | `RawMcsLock` | MCS queue spin lock (implements `RawMutex`) |
| `mutex.rs` | `DLock2Mutex<T, I, F>` | `std::sync::Mutex` wrapper |
| `uscl.rs` | `DLock2USCL<T, I, F>` | U-SCL fairlock (C FFI wrapper) |

### C Reference Implementations

Wrapped in `c_binding/`: `CFlatCombining<T, F, I>`, `CCCSynch<T, F, I>`.

## FC-PQ: Key Algorithm

`FCPQ<T, I, PQ, F, L>` accepts only the library's sealed priority-queue adapters:
- `BTreeSet<UsageNode<'static, I>>` — O(log N) insert/pop-min
- `BinaryHeap<Reverse<UsageNode<'static, I>>>` — O(log N) insert/pop-min
Downstream safe custom `SequentialPriorityQueue` implementations are deliberately
forbidden: the queue holds references into per-lock thread-local nodes, and a
queue that copies an entry outside the lock could dereference it after drop.
The apparent `'static` on an entry is internal to the lock, not a promise to
users. Stop admissions and wait for **every** `lock()` call to return before
destroying a lock; completion of a delegate alone is not quiescence. Recursion
on the same lock and unwinding out of a delegate are unsupported.

**Combining loop** (`combine()`):
1. Drain `waiting_nodes` ring buffer into `job_queue`, initializing newcomers to running-average usage
2. Pop min-usage node from PQ, execute delegate, accumulate CS time into `usage`, push back
3. Repeat up to H=64 times per combining pass
4. Completed nodes (already served but not yet re-requested) are buffered and eventually deactivated

**Fairness scope**: accumulated usage drives selection among scheduler-visible
entries; this alone does not bound request latency or realized service shares.
Publication, eligibility, combining-pass budgets and callback duration also matter.

## Common Structure

Each delegation lock module typically contains:
- `lock.rs` — Main struct + `DLock2<I>` impl
- `node.rs` — Per-thread node struct (stored in `ThreadLocal<SyncUnsafeCell<Node<I>>>`)

Nodes contain: request data (`MaybeUninit<I>`), completion flag (`AtomicBool`), usage counter (`AtomicU64`), active flag.

FC and FC-PQ publish requests through a persistent interior-mutable payload
cell and release/acquire completion flag. The requester owns the input before
publication and the returned output after completion; combiners can still hold
shared references to the node while the requester starts a later request.
The node's stable address is retained until quiescent lock destruction. Only
the node's thread-local owner reads/writes its combining-time statistic; other
threads may hold shared references to the containing node. The FC-PQ admission
ring likewise shares entries across producer/consumer, with the valid flag
transferring ownership of the slot's interior-mutable value.

## Spin-then-park waiters (`spin_park` feature)

FC and FC-PQ waiters spin on `node.complete` and never sleep, which collapses
under oversubscription (finding 002). With the `spin_park` cargo feature
(`libdlock/spin_park`, forwarded by `dlock` and `upscaledb-bridge`) a waiter
spins for `park::SPIN_BEFORE_PARK` and then parks on a per-node futex
(`park.rs`, based on `parker/block_parker.rs`). Default builds are unchanged.

Waiter, per request (`lock()`):

1. `complete = false` (SeqCst), enroll if `!active` (FC: list push; FC-PQ:
   `waiting_nodes.push`, visible at its `tail.fetch_add`).
2. `combiner_lock.try_lock()`: on success run a pass and release (step 5).
3. Otherwise spin 8 x `Backoff::spin()` on `complete`, then go to 1
   (re-check enrollment, `try_lock` again) while the budget lasts.
4. Budget exhausted, immediately after a failed `try_lock`:
   `parked += 1`; `park = PARKED` (SeqCst); `fence(SeqCst)`; re-read
   `complete` (SeqCst; done: unpark); re-read `active` (retired: unpark, go
   to 1); one last `try_lock` (success: unpark, become combiner);
   `futex_wait(park == PARKED)`. On return, `park = EMPTY`; take the result
   if complete, else go to 1.

Combiner, per served request: `complete = true` (Release); then, after the
next delegate has run (or at the end of the pass), `fence(SeqCst)`; if
`park == PARKED`, CAS it to `NOTIFIED`, `parked -= 1`, `futex_wake`. The
deferral lets the result stores drain before the fence, so the fence does not
stall on the line the waiter is spinning on (measured: an immediate `xchg`
cost ~11% throughput at 8 threads on 8 CPUs).

Every release of `combiner_lock` goes through `release_combiner()`:
`unlock(); loop { fence(SeqCst); if parked == 0 break; if !try_lock() break;
pass; unlock() }`. A holder that unlocks without combining (the planned
FC-PQ uncontended fast path) must use it too.

Why there is no lost wakeup and no stall:

- Completion: `PARKED (SeqCst); fence; load complete` against
  `complete = true (Release); ...; fence; load park` is a Dekker pair under
  the C++ SeqCst-fence rule, whatever the distance between the store and the
  fence; the combiner's wake changes the futex word, so a
  `futex_wait(PARKED)` that races with it returns instead of sleeping.
- Progress: `parked += 1; PARKED; fence; try_lock` against `unlock; fence;
  load parked` is a Dekker pair. Either the waiter's last `try_lock` sees the
  lock free (it combines itself), or the unlocker sees `parked >= 1` and
  re-acquires; a holder that wins the lock in between inherits the check at
  its own release. The pre-park `try_lock` alone is not enough (the holder's
  pass may already have missed the enrollment) and an unlocker-side re-scan
  alone is not enough (it can run before the enrollment); both halves are
  required.
- Enrollment: retirement of a node (FC cleanup unlink; FC-PQ dropping a
  complete node from the queue) is `active = false` (SeqCst), re-read
  `complete` (SeqCst), and if a request is pending, `CAS(active false->true)`
  to re-enroll it (FC: relink at head; FC-PQ: push back into the job queue).
  The owner's `complete = false` (SeqCst) followed by `active.load(SeqCst)`
  and the same CAS make exactly one side enroll, so a waiter never parks with
  an unenrolled request. Unit tests shrink the spin budget to 5 us so that
  these paths are exercised; `dlock2_unit_test::idle_holder_release` covers
  a holder that unlocks without combining while waiters are parked.
