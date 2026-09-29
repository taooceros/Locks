# Finding 002: DLock2 Spin-Wait Causes Collapse Under Oversubscription

**Date:** 2026-09-26 (trimmed 2026-09-28)
**Status:** Diagnosed from existing data (code audit + re-reading of 2026-09-23 results). Parking (E0(a)/E0(a')) is shelved, so this is a stated limitation: every experiment keeps threads <= CPUs.
**Component:** DLock2 waiter loop (`crates/libdlock/src/dlock2/fc/lock.rs`, `crates/libdlock/src/dlock2/fc_pq/lock.rs`)
**Severity:** Waiting-policy limitation (not a delegation-cost result)
**Machine:** packed 4-CPU placement, UpScaleDB integration (`experiment/upscaledb-fc-pq-integration@dcbfacb:plan/2026-09-23/upscaledb-integration-plan.md`)

This finding is the rationale for the threads <= CPUs rule. The measurements
from the oversubscribed runs that first showed the collapse are withdrawn
(see the [evidence index](../evidence/README.md#withdrawn)); the mechanism
below is from the code and does not depend on them.

## Observation

On a packed 4-CPU placement, FC-family backends collapse as soon as there are
more workers than CPUs, while the pthread-mutex backends do not.

## Mechanism

The DLock2 waiter never sleeps. In
`crates/libdlock/src/dlock2/fc/lock.rs` (waiter branch of `lock`, lines
195-212) a thread that fails `combiner_lock.try_lock()` does:

```rust
let backoff = Backoff::new();
let mut count: u32 = 8;
loop {
    if node.complete.load(Acquire) {
        break 'outer;
    }
    backoff.spin();
    count = count.wrapping_sub(1);
    if count == 0 {
        continue 'outer;
    }
}
```

`Backoff::spin()` only issues pause instructions; it never yields and never
touches a futex. Every 8 iterations the waiter goes back to `'outer` and
retries the combiner lock. `crates/libdlock/src/dlock2/fc_pq/lock.rs` lines
340-358 have the identical loop. DLock2 has no Parker at all:
`crates/libdlock/src/parker/README.md` states that DLock2 locks use
spin-backoff (`crossbeam::Backoff`) directly instead of the Parker trait.

With 8 workers on 4 CPUs there are 7 spinners for one combiner. The
scheduler gives each runnable thread a slice; whenever the single combiner is
preempted, the 4 running threads are all spinners polling `complete`, and no
request can make progress until the combiner is rescheduled. Every CPU stays
busy, mostly on `pause`.

pthread mutex waiters futex-sleep, so the holder always has a CPU.

## Confirmation from the same data

- 4W on 4 CPUs (no oversubscription): FC-PQ 2.039x vs native, bridge -> FC
  2.531x. Once every thread has its own CPU the same code wins.

## Implications

- No oversubscription result is reported: every experiment keeps threads <=
  CPUs. A `BlockParker` exists in `crates/libdlock/src/parker/block_parker.rs`,
  but only DLock1 uses it, and wiring it into DLock2 is shelved.
- CPU-s comparisons with threads <= CPUs still measure the spin policy. This
  explains the ~4x CPU-s gap vs U-SCL at P8 in the joined study (FC 8.43 /
  FC-PQ 8.42 CPU-s vs USCL 2.07 CPU-s, see finding 003): U-SCL waiters block,
  DLock2 waiters spin.

## Status

Diagnosed 2026-09-26 from existing data. Parking is shelved; the limitation is
stated in `README.md` instead of fixed.
