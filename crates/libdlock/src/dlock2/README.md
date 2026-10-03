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

**Low-contention fast path** (E0(b) ablation; cargo features of `libdlock`,
all default-off; [plan](../../../../plan/2026-09-27/e0b-fcpq-fast-path.md)):

| Feature | Effect |
|---------|--------|
| `fcpq_cached_tid` | The ring tie-breaker (thread id) is cached in the node at creation instead of `current().id()` per enrollment (D6) |
| `fcpq_fast_path` | `lock()` tries the combiner lock before publishing its request. If the caller's node is inactive and both the PQ and the admission ring are empty, the caller runs its own request and charges usage, `total_usage` and `total_served` with the same timestamps as `combine()` (D1, D2, D4) |
| `fcpq_fast_path_notime` | The same bypass without timestamps or usage charge; isolates timestamp cost and is fairness-incorrect by design. Mutually exclusive with `fcpq_fast_path` |
| `fcpq_fast_path_stat` | Per-thread hit counter: `FCPQ::get_fast_path_hits()` (calling thread) and `FCPQ::fast_path_hits()` (sum, exact once callers have joined) |
| `combiner_pass_stat` | Combiner pass counters for FC and FC-PQ: `pass_stats()` returns passes, empty passes, bodies, combiner changes (the combiner differs from the previous pass's), cap violations and a bodies-per-pass histogram, taken under the combiner lock. Default-off; used only by a separate stats build (plan `2026-09-29/fcpq-pass-length-ablation.md`) |

**Pass length (`PassCap`).** `combine()` pops at most `H` queue entries per pass.
`FCPQ::new` keeps `PassCap::Fixed(64)`; `FCPQ::with_pass_cap(data, delegate, cap)` selects
`Fixed(n)` (n >= 1) or `Active`, the number of entries in the queue when the pass starts
(after the announcement ring has been drained), so one combiner runs about one body per
enrolled node, as FC's single sweep does. A pop of a completed node counts against the cap
without running a body, so a pass runs at most `cap` bodies. The cap is a runtime field read
once per pass (a const generic would change the lock's type at every use and cannot express
`Active`); the unit tests `pass_cap_tests` check that no pass exceeds its cap.

The gate runs once per request, before the request is written to the node
(payload, `complete=false`). A combiner that still holds the node from an
earlier request would serve a published request, so a later bypass could run
it twice. A holder that finds other work pending publishes and combines if its
node is still enrolled. Otherwise it releases the lock before enrolling,
because the ring may be full and only a lock holder drains it. Arrival means
ring publication (`tail.fetch_add`), not `active=true` (D5). Before
publication a newcomer is invisible, and any number of fast-path CSs may run.
Once its increment is visible, at most one fast-path CS, already past its
gate, finishes ahead of it.

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
