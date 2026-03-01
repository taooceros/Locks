# dlock/ — DLock1: Callback-Based Delegation Locks

Older API generation. User passes a closure that receives a `DLockGuard<T>` for direct mutable access to the shared data.

## Trait

```rust
pub trait DLockDelegate<T>: Send + Sync {
    fn apply(&mut self, data: DLockGuard<T>);
}

pub trait DLock<T> {
    fn lock<'a>(&self, f: impl DLockDelegate<T> + 'a);
}
```

`DLockGuard<T>` wraps a `&SyncUnsafeCell<T>` and implements `Deref`/`DerefMut`.

Any `FnMut(DLockGuard<T>) + Send + Sync` automatically implements `DLockDelegate<T>`.

## Enum Dispatch

Three-level enum hierarchy via `#[enum_dispatch]`:

- `BenchmarkType<T>` — top-level: `SpinDLock(DLockType<T, SpinParker>)` | `BlockDLock(DLockType<T, BlockParker>)` | `OtherLocks(ThirdPartyLock<T>)`
- `DLockType<T, P>` — delegation locks parametrized by Parker: FC, FCBan, FCBanSlice, FCSLNaive, FCSL, CCSynch, CCBan, RCL
- `ThirdPartyLock<T>` — non-delegation baselines: Mutex, SpinLock, USCL

## Variants

| Module | Type | Description |
|--------|------|-------------|
| `fc/` | `FcLock<T, L, P>` | Flat Combining — combiner traverses thread-local node list |
| `fc_fair_ban/` | `FcFairBanLock<T, L, P>` | FC + TSC-based banning (overshare detection) |
| `fc_fair_ban_slice/` | `FcFairBanSliceLock<T, L, P>` | FC + banning with combiner time slicing |
| `fc_sl/` | `FCSL<T, L, P>` | FC with concurrent skip-list ordered by usage |
| `fc_sl_naive/` | `FCSLNaive<T, L, P>` | FC with naive (non-concurrent) skip-list |
| `ccsynch/` | `CCSynch<T, P>` | CCSynch — FIFO linked-list job queue |
| `ccsynch_fair_ban/` | `CCBan<T, P>` | CCSynch + banning |
| `rcl/` | `RclLock<T, P>` | Remote Core Locking — dedicated combiner thread |
| `guard.rs` | `DLockGuard<T>` | Guard type for safe mutable access |
| `mutex_extension.rs` | — | `DLock<T>` impl for `std::sync::Mutex<T>` |

Each variant module typically contains `lock.rs` (implementation) and `node.rs` (per-thread node struct).
