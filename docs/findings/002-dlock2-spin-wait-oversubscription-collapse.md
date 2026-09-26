# Finding 002: DLock2 Spin-Wait Causes Collapse Under Oversubscription

**Date:** 2026-09-26
**Status:** Diagnosed from existing data (code audit + re-reading of 2026-09-23 results); fix not implemented
**Component:** DLock2 waiter loop (`crates/libdlock/src/dlock2/fc/lock.rs`, `crates/libdlock/src/dlock2/fc_pq/lock.rs`)
**Severity:** Waiting-policy limitation (not a delegation-cost result)
**Machine:** packed 4-CPU placement, UpScaleDB integration (`experiment/upscaledb-fc-pq-integration@dcbfacb:plan/2026-09-23/upscaledb-integration-plan.md`)

## Observation

On a packed 4-CPU placement, FC-family backends collapse as soon as there are
more workers than CPUs, while the pthread-mutex backends do not.

Fixed work (400K find + 400K insert), paired speedup vs native:

| Workers on 4 CPUs | FC-PQ vs Native |
|---|---:|
| 8 | 0.494x |
| 16 | 0.209x |

120 s duration mode, 4 finders + 4 inserters on 4 CPUs (packed primary means):

| Backend | find/s | insert/s | CPU-s / 120 s |
|---|---:|---:|---:|
| Native | 764,690 | 573,811 | 127.4 |
| Bridge mutex | 756,095 | 563,655 | 127.2 |
| FC | 292,665 | 272,638 | 479.9 |
| FC-PQ | 282,031 | 229,351 | 479.9 |

480 CPU-s is 4 CPUs x 120 s fully busy: both delegation backends pin every
CPU for the whole window while finishing well under half the native work.

Stage decomposition, packed 8W fixed (paired means, ranges from source):

| Step | Duration change |
|---|---:|
| refactored -> bridge mutex | +10.5% [1.7, 18.8] |
| bridge mutex -> FC | +54.2% [41.4, 66.9] |
| FC -> FC-PQ | +25.1% [18.1, 33.4] |

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
request can make progress until the combiner is rescheduled. The 480 CPU-s
figure is exactly this: every CPU busy, mostly on `pause`.

pthread mutex waiters futex-sleep, so the holder always has a CPU. That is
why native and bridge mutex stay at ~127 CPU-s and ~760K find/s in the same
configuration.

The bridge/FFI stage costs at most ~10% (refactored -> bridge +10.5%), so the
collapse is the waiting policy, not delegation per se.

## Confirmation from the same data

- 4W on 4 CPUs (no oversubscription): FC-PQ 2.039x vs native, bridge -> FC
  2.531x. Once every thread has its own CPU the same code wins.
- 1W: FC 0.227 s vs native 0.219 s (~+4%); FC-PQ 0.281 s (~+28%). Without
  contention the delegation path itself is cheap for FC; FC-PQ carries a
  constant per-request tax.

## Implications

- Spin-then-park must be wired into DLock2 before any CPU-efficiency or
  oversubscription claim is made. A `BlockParker` already exists in
  `crates/libdlock/src/parker/block_parker.rs`, but only DLock1 uses it.
- The same policy explains the ~4x CPU-s gap vs U-SCL at P8 in the joined
  study (FC 8.43 / FC-PQ 8.42 CPU-s vs USCL 2.07 CPU-s, see finding 003):
  U-SCL waiters block, DLock2 waiters spin.
- Until the fix lands, oversubscribed and CPU-s comparisons measure the
  spin policy, not the fairness-by-switching-requests mechanism.

## Status

Diagnosed 2026-09-26 from existing data; fix not implemented.
