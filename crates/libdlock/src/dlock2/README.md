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

`FCPQ<T, I, PQ, F, L>` is generalized over the priority queue type:
- `BTreeSet<UsageNode>` — O(log N) insert/pop-min
- `BinaryHeap<Reverse<UsageNode>>` — O(log N) insert/pop-min

**Combining loop** (`combine()`):
1. Drain `waiting_nodes` ring buffer into `job_queue`, initializing newcomers to running-average usage
2. Pop min-usage node from PQ, execute delegate, accumulate CS time into `usage`, push back
3. Repeat up to H=64 times per combining pass
4. Completed nodes (already served but not yet re-requested) are buffered and eventually deactivated

**Fairness bound**: `|U_i - U_j| <= C_max` where `C_max` is the maximum critical section duration.

## Common Structure

Each delegation lock module typically contains:
- `lock.rs` — Main struct + `DLock2<I>` impl
- `node.rs` — Per-thread node struct (stored in `ThreadLocal<SyncUnsafeCell<Node<I>>>`)

Nodes contain: request data (`MaybeUninit<I>`), completion flag (`AtomicBool`), usage counter (`AtomicU64`), active flag.
