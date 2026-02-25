# c/ — C Reference Implementations

Reference C lock implementations used for cross-language comparison. Compiled by `rust/lib-dlock/build.rs` via the `cc` crate and accessed through `bindgen`-generated FFI bindings.

## Compiled Sources

| Directory | Implementation | Wrapped in Rust as |
|-----------|---------------|--------------------|
| `CCsynch/` | CCSynch (FIFO combining) | `CCCSynch<T, F, I>` |
| `FlatCombining/original/` | Flat Combining | `CFlatCombining<T, F, I>` |
| `u-scl/` | U-SCL fairlock | `USCL<T>` / `DLock2USCL<T, I, F>` |

## Not Compiled (reference only)

| Directory | Description |
|-----------|-------------|
| `RCL/` | Remote Core Locking (Rust reimplementation exists) |
| `ticket/` | Ticket lock |
| `libpqueue/` | Priority queue library |
| `shared/` | Common headers/utilities (included via `-I` flag) |
| `unit_test/` | C unit tests |

## Build Configuration

Defined in `rust/lib-dlock/build.rs`:
- `CYCLE_PER_US = 2400` (assumed CPU frequency for timing)
- `FC_THREAD_MAX_CYCLE = CYCLE_PER_MS` (FC combiner time limit)
- Optimization: `-O2`
- Warnings suppressed
