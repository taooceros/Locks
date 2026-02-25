# parker/ — Thread Parking Abstraction

Provides the `Parker` trait used by delegation locks to manage waiting threads. The choice of parker affects latency/CPU tradeoff: spin-waiting is lower latency but burns CPU; futex-based blocking frees CPU but adds wake-up latency.

## Trait

```rust
pub enum State { Empty, Parked, Prenotified, Notified }

pub trait Parker: Debug + Default + Send + Sync {
    fn wait(&self);
    fn wait_timeout(&self, timeout: Duration) -> Result<(), ()>;
    fn wake(&self);
    fn state(&self) -> State;
    fn reset(&self);
    fn prewake(&self);     // transition to Prenotified (avoids lost wakeup)
    fn name() -> &'static str;
}
```

## Implementations

| File | Type | Strategy |
|------|------|----------|
| `spin_parker.rs` | `SpinParker` | Spin-loop with `__rdtscp` backoff. Low latency, high CPU usage. |
| `block_parker.rs` | `BlockParker` | Linux futex (`FUTEX_WAIT`/`FUTEX_WAKE`). OS-level blocking, lower CPU. |

## Usage

DLock1 locks are parameterized by `P: Parker` (e.g., `FcLock<T, L, SpinParker>`). The benchmark harness tests both parker types via the `BenchmarkType::SpinDLock` / `BlockDLock` variants.

DLock2 locks use spin-backoff (`crossbeam::Backoff`) directly instead of the Parker trait.
