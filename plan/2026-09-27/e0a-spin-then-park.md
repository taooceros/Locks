# E0(a): Spin-then-park waiters for FC and FC-PQ

Approved: 2026-09-27 (user instruction). Implemented directly in the
`Locks-e0` jj workspace on change `E0: engineering prerequisites`.

## Goal

DLock2 waiters (`fc/lock.rs`, `fc_pq/lock.rs`) never sleep: a thread whose
request is pending polls `node.complete` with `Backoff::spin()` forever.
With more workers than CPUs the combiner is preempted by spinners and
throughput collapses (finding 002: 0.494x at 8 workers on 4 CPUs, 480 CPU-s
for 120 s). E0(a) adds a bounded spin followed by a futex park, behind a
cargo feature, without changing the default build.

Non-goals: FC-PQ per-request tax and uncontended fast path (E0(b), separate
plan `plan/2026-09-27/e0b-fcpq-fast-path.md` in the main checkout), other
locks, prewake of the next waiter, harness changes beyond the feature flag.

## Feature

`spin_park` on `libdlock`, forwarded by the root `dlock` crate and by
`upscaledb-bridge` (`spin_park = ["libdlock/spin_park"]`). Disabled by
default; with it disabled the waiter loop, node layout and orderings are the
pre-existing ones.

## Design

State added under the feature:

- `Node.park: ParkSlot` (`dlock2/park.rs`), a `linux_futex::Futex<Private>`
  word with values `EMPTY`, `PARKED`, `NOTIFIED`. Only the owner writes
  `EMPTY`/`PARKED`; a combiner moves `PARKED -> NOTIFIED` with a CAS.
- `FC.parked` / `FCPQ.parked`: `CachePadded<AtomicU32>`, number of waiters
  that have published `PARKED` and have not yet been resolved.
- `SPIN_BEFORE_PARK` (`park.rs`): spin budget per request, checked once per
  8-spin round with `Instant::now()` (lazily armed after the first round, so
  an uncontended request never reads the clock). Default 100 us; build-time
  override `DLOCK_SPIN_BEFORE_PARK_US=<us>`; 5 us under `cfg(test)` so the
  unit tests exercise the park paths.

### Waiter (owner of the node), per request

```
publish input; complete.store(false, SeqCst)
loop 'outer:
  enroll if !active (FC: list push; FC-PQ: waiting_nodes.push, visible at tail.fetch_add)
  if combiner_lock.try_lock(): combining pass; release (see unlocker); done if complete
  else if SPIN_BEFORE_PARK elapsed (enrollment check and failed try_lock just happened):
          parked += 1  (SeqCst)                 -- counted before the state is visible
          park.state = PARKED (SeqCst); fence(SeqCst)
          if complete            -> unpark, take result
          if !active             -> unpark, retry 'outer (re-enroll)
          if try_lock()          -> unpark, become combiner (same path as above)
          futex_wait(state == PARKED) until state != PARKED
          state = EMPTY; complete ? take result : retry 'outer (spurious)
  else: 8 x Backoff::spin() polling complete, then retry 'outer (existing loop)
```

`unpark` is `CAS(PARKED -> EMPTY)`: on success the waiter does `parked -= 1`;
on failure a combiner already moved it to `NOTIFIED` and did the decrement.

### Combiner, per served request

```
write result; complete.store(true, Release); remember node as `unwoken`
... after the next delegate has run (or after the pass):
fence(SeqCst)
if unwoken.park.state == PARKED and CAS(PARKED -> NOTIFIED) succeeds:
    parked -= 1; futex_wake(state, 1)
```

The fence pair (`PARKED; fence; load complete` vs `complete=true; ...; fence;
load state`) is a Dekker under the C++ SeqCst-fence rule, whatever the
distance between the store and the fence: either the waiter sees the result
before sleeping or the combiner sees `PARKED` and changes the word, so
`futex_wait(PARKED)` returns `EAGAIN` or is woken. The wake always changes
the futex word; a bare `futex_wake` after a plain load would be lost if it
raced with the waiter's `futex_wait` entry. The check is deferred past the
next delegate because an immediate store-load barrier (`xchg`/`mfence` right
after the result store) stalls the pass until the result line's RFO
completes against the spinning waiter: measured -11% throughput at 8 threads
on 8 CPUs; deferred, -2 to -4%. Cost of the deferral: a parked waiter's wake
arrives one critical section later.

### Unlocker (every release of `combiner_lock`)

```
unlock(); loop { fence(SeqCst); if parked == 0 break; if !try_lock() break; combining pass; unlock() }
```

`unlock; fence; load parked` vs the waiter's `parked += 1; PARKED; fence;
try_lock` is the second Dekker: either the waiter's last `try_lock` sees the
lock free (it becomes the combiner and serves itself), or the unlocker sees
`parked >= 1` and either re-acquires and combines, or loses `try_lock` to a
new holder who inherits the obligation at its own release. A holder whose
lock value the waiter's failed `try_lock` observed always reaches this check
after that observation, so a parked waiter with an enrolled request is never
left with no combiner. Both halves are required: the waiter's pre-park
`try_lock` alone stalls when a holder's pass already missed the enrollment,
and an unlocker-side re-check alone can run before the waiter's enrollment
and both miss.

The counter replaces "re-walk the list / re-read the ring after unlock": it
is O(1), written only on park/unpark events, and it also covers
re-requests by already-enrolled nodes, which a "did the list head change"
check would miss.

### Enrollment must survive deactivation (the third Dekker)

Both locks retire nodes: FC unlinks nodes older than `CLEAN_UP_AGE` passes,
FC-PQ drops nodes it finds complete from the priority queue (`active=false`).
Today a retired-while-pending node is harmless because the spinning owner
re-checks `active` every round and re-enrolls. A parked owner cannot, and the
unlocker's re-combine would never find the node (livelock: `parked >= 1`, no
enrolled request). Under the feature the retirement is a CAS decision:

```
owner:     complete.store(false, SeqCst); if !active.load(SeqCst) && CAS(active false->true): enroll
retirer:   (FC: unlink) active.store(false, SeqCst)
           if !complete.load(SeqCst) && CAS(active false->true): re-enroll it myself
           (FC: relink at head; FC-PQ: push back into the job queue)
```

Either the owner observes `active=false` and enrolls, or the retirer observes
the pending request; the CAS lets exactly one side enroll. Invariant: a
waiter that reaches the park path has an enrolled, unserved request.

### Forthcoming FC-PQ uncontended fast path (E0(b))

A holder that executes its own request without combining releases the lock
via the same `release_combiner()` (unlock + fenced `parked` check). A waiter
that enrolled after that holder's emptiness check and then parked is covered
by the unlocker Dekker above; no additional rule is needed. E0(b) must call
`release_combiner()` for every release, not `combiner_lock.unlock()`.

## Evaluation

- `cargo build --release --workspace` and `cargo test --release --workspace`
  with and without `--features spin_park`.
- New test `dlock2_unit_test::idle_holder_release` (FC, FC-PQ heap, FC-PQ
  btree): a holder takes `combiner_lock`, does nothing for 5 ms (1000x the
  test spin budget) or 20 us, releases via `release_combiner()`; 1 and 3
  waiters must complete with exact counter responses, 40 rounds. Negative
  control: with the post-unlock `parked` check disabled the FC variant times
  out (stall), so the test detects the failure it is written for.
- Smoke, feature off vs on: `taskset -c` 4 CPUs, `d-lock2 -t 8`
  counter-proportional with `fc` and `fc-pq-b-heap`; throughput and
  `/usr/bin/time` user+sys CPU; repeated runs, no hang. Non-oversubscribed
  run (8 threads on 8 CPUs) to bound the regression.

## Results (2026-09-27)

Machine: 2x Xeon Gold 6438M, kernel 6.17.7, shared; runs pinned to node-1
cores 48-55 (siblings idle) while a sibling agent's 32-thread benchmark ran
on cores 0-31 (load average 5-24 during the runs). `counter-proportional`,
`--cs 1000 --non-cs 0`, 1 s warmup + 3 s measured, one lock per process,
`/usr/bin/time` user+sys over the 4 s process. Mops/s = total loop count /
cs / 3 s. Binaries: default features vs `--features spin_park`
(`SPIN_BEFORE_PARK` = 100 us unless noted).

| Config | Lock | Mode | Mops/s (runs) | CPU-s user+sys |
|---|---|---|---|---|
| 8 threads / 4 CPUs | fc | default | 0.78, 0.78, 0.72, 0.76 | 16.05 each |
| 8 threads / 4 CPUs | fc | spin_park | 0.78, 0.78, 0.77, 0.80 | 15.9 |
| 8 threads / 4 CPUs | fc-pq-b-heap | default | 0.71, 0.71, 0.71, 0.68 | 16.04 |
| 8 threads / 4 CPUs | fc-pq-b-heap | spin_park | 0.74, 0.73, 0.74, 0.74 | 15.4 |
| 8 threads / 4 CPUs | fc | spin_park, 20 us | 0.79, 0.75 | 15.7 |
| 8 threads / 4 CPUs | fc-pq-b-heap | spin_park, 20 us | 0.91, 0.86 | 15.1 |
| 8 threads / 4 CPUs, cs=20000 | fc | default | 0.048, 0.048 | 16.05 |
| 8 threads / 4 CPUs, cs=20000 | fc | spin_park | 0.057, 0.057 | 15.1 |
| 8 threads / 4 CPUs, cs=20000 | fc-pq-b-heap | default | 0.048, 0.048 | 16.04 |
| 8 threads / 4 CPUs, cs=20000 | fc-pq-b-heap | spin_park | 0.052, 0.053 | 13.5 |
| 8 threads / 8 CPUs | fc | default | 1.52, 1.52, 1.51, 1.51 | 32.0 |
| 8 threads / 8 CPUs | fc | spin_park | 1.48, 1.49, 1.51, 1.49 | 32.0 |
| 8 threads / 8 CPUs | fc-pq-b-heap | default | 1.55, 1.53, 1.55, 1.56 | 32.0 |
| 8 threads / 8 CPUs | fc-pq-b-heap | spin_park | 1.50, 1.49, 1.49, 1.49 | 30.7 |
| 8 threads / 8 CPUs | fc | spin_park, 20 us | 1.50, 1.51 | 32.0 |
| 8 threads / 8 CPUs | fc-pq-b-heap | spin_park, 20 us | 1.36, 1.37 | 25.7 |

An earlier build with the immediate (non-deferred) combiner-side `xchg`
measured 1.29 vs 1.45 Mops/s (fc, 8/8, -11%) and, under the heavier
concurrent load at that time, fc-pq-b-heap 8/4 at 0.76 Mops/s with 11.1
CPU-s (vs 0.69 / 16.0 default). No run hung (20 oversubscribed process
runs with the feature, all 4.0 s wall).

Reading: with cs=1000 an FC pass at 8 threads is ~5-8 us, far below the
100 us budget, so waiters only park when the combiner is descheduled; CPU
time barely moves and throughput is within noise. FC-PQ, whose responses are
more variable, parks more (sys time > 0) and gains 4-10% oversubscribed. With
a 20 us budget FC-PQ gains 22-28% oversubscribed but loses 12% when every
thread has its own CPU, so 100 us stays the default; E1/E3 can sweep it via
`DLOCK_SPIN_BEFORE_PARK_US`. With longer critical sections (cs=20000, pass
> budget) both locks park under oversubscription: +19% (fc) and +10% (fc-pq)
throughput with 6-16% less CPU. Non-oversubscribed regression with the
default budget: 1-3% (fc), 3-4% (fc-pq).

Note: `dlock2_unit_test::fc_sl::threads_8` (FC-SL, untouched by this
change) timed out three times in serial runs while the host load average
was 17-38, in both feature modes across attempts; pinned to idle cores it
passed 10/10 in each mode. Pre-existing load sensitivity, not introduced
here.

## Risks

- Wake latency: a parked waiter's next request is delayed by futex wake
  (5-50 us) plus a combiner-side syscall per parked wake; if the pass length
  exceeds `SPIN_BEFORE_PARK` on a non-oversubscribed run, throughput drops.
  Mitigation: budget above typical pass length; E1 may tune it.
- Extra per-request cost with the feature on: one `xchg` for the SeqCst
  `complete=false`, one `mfence` per served request (combiner-side Dekker),
  one `mfence` + load per release. All feature-gated.
- `parked` is a shared line written on every park/unpark; under heavy
  parking it bounces between combiner and parkers. Acceptable: those paths
  already pay a syscall.
- Spurious wakes (stale `NOTIFIED` from a delayed combiner CAS) are
  tolerated by re-checking `complete` after every return from `futex_wait`.
- Priority-inversion style stalls under extreme oversubscription remain
  possible for the combiner itself (it never parks); out of scope.
