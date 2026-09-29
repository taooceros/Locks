# E0(b): FC-PQ low-contention fast path

Status: Approved: 2026-09-27 (user instruction, ablation in workspace
e0b-fastpath). Implemented behind default-off features (see Implementation);
benchmarks pending. Part of E0(b) in [TODO.md](../../TODO.md).
Timestamp choice (rdtsc vs rdtscp, end reuse) is measured separately in
`.worktree/tsc-accuracy/RESULTS.md` and is out of scope here.

## Problem

FC-PQ pays the full delegation protocol even when nobody else is waiting.
Target: FC-PQ/FC >= 0.95 at 1 worker (measured: +28% tax). Per request at
1 worker, `lock()` (`fc_pq/lock.rs:321-364`) and `combine()` (`:162-310`) do:

1. `push_if_unactive`: the node was deactivated at the end of the previous
   pass (`:291-297`), so every request re-enrolls through the ring buffer
   (atomic push + `current().id()`, which clones and drops a `Thread` handle).
2. `try_lock`, then a combine pass: pass counter, ring drain, PQ push, pop,
   peek + prefetch, two `__rdtscp`, PQ re-push, a second pop that finds the node
   complete, 4-slot buffer, drain, two release stores.

None of this changes who is served when there is exactly one requester.

## Invariant the fast path must keep

A request may bypass the PQ only if no other request is pending. Pending
requests are visible in two places:

- the ring `waiting_nodes` (newly enrolled nodes), and
- **active nodes already in the PQ**, which publish a new request by setting
  `complete=false` without touching the ring (`push_if_unactive` returns early).

So `waiting_nodes.empty()` alone is insufficient; the PQ must hold no active
node either. Usage must still be charged to the caller's node, so FC-PQ's
ordering among later contenders is unchanged.

## Decisions

### D1. Where to test for the fast path

`lock()` already issues `try_lock` on every outer-loop iteration (`:335`) and
spinners re-enter that loop every 8 backoffs (`:354-356`). Moving `try_lock`
ahead of `push_if_unactive` therefore *replaces* the existing CAS rather than
adding one; a combiner-written hint would add a shared read for nothing.

**Decision:** try-lock first, publish only on failure or when pending work is
found under the lock. No hint.

### D2. Emptiness check under the lock

Every node in `job_queue` is active: `active` is cleared only when a popped
node is found complete and drained (`:277-279`, `:292-294`). A complete node
retained in the PQ whose owner re-issues (`complete=false`, no ring push
because `active` is still set) is genuinely pending, so a separate
active-node counter would have to count it anyway.

**Decision:** gate =
`!node.active && job_queue.peek().is_none() && waiting_nodes.empty()`
(`buffer.rs:106`). No new state. Stale complete nodes retained across a pass
cost one missed fast path; the next combine pops and deactivates them. The
`!node.active` conjunct is implied by the other two but stays explicit as the
readable invariant.

### D3. Keeping the node enrolled vs re-enrolling

- a. Fast path leaves the node inactive: no ring push at all while
  uncontended. When contention appears, the node enrolls once via the slow path.
- b. Keep nodes enrolled across passes and retire idle ones by age (FC's
  `clean_unactive_node`). Helps moderate contention too, but makes D2's check
  fail whenever any retained node exists.

**Recommend a** for the fast path; evaluate b separately as a slow-path
optimization.

### D4. Accounting on the fast path

Under the combiner lock with the node inactive, the owner may update
`node.usage` directly (the combiner stores it with release at deactivation,
`:293`; the lock acquire orders it). Also update `total_usage`/`total_served`
so newcomer initialization (`:197-202`) stays correct.

**Decision:** keep both timestamps on the fast path. Usage accumulated while
uncontended is what ranks a long-CS thread correctly once contention appears;
skipping or averaging it is the accounting error `analysis/logp/fairness.tex`
bounds. The residual per-request tax is then ~2 timestamps + 1 CAS + 1 unlock
store; whether the timestamp is `rdtscp` or `rdtsc` follows the tsc-accuracy
result. Sampling every k requests (TODO E0(b) wording) is orthogonal and can
be layered later if the ratio still misses 0.95.

### D6. Independent slow-path trim

`push_node` (`:150`) calls `current().id()` per enrollment: a `Thread` Arc
clone+drop, two locked RMWs. Cache the `u64` in `Node::new`. Applies under
contention as well, so measure it separately from the fast path.

### Sketch

As implemented. The draft published first and gated inside the retry loop;
see I0-I3.

```
lock(data):
  if try_lock():                             // D1; reused as the loop's first CAS
    if !node.active && job_queue.peek().is_none() && waiting_nodes.empty():
      begin=ts; r = delegate(data); end=ts   // D4 (notime: no ts, no charge)
      u = node.usage, or running average if u == 0 and served > 0   // I4
      node.usage = u + end-begin; total_usage += end-begin; total_served += 1
      unlock(); return r                     // D5: one CS of barging
    if node.active: acquired = true          // still enrolled: combine now
    else: unlock(); acquired = try again     // I2: never enroll holding the lock
  else: acquired = false
  write data; complete=false                 // I0: publish only after a miss
  loop:                                      // unchanged slow path
    push_if_unactive(node)
    if (first iteration ? acquired : try_lock()):
      combine(); unlock(); if complete: break
    else:
      spin 8x backoff on complete; retry
```

### D5. Barging bound

Arrival is ring publication: a newcomer B is pending from its
`tail.fetch_add` in `waiting_nodes.push` (`buffer.rs:72`), not from the
`active=true` that `push_node` stores first. Before that increment B is
invisible to the gate, so any number of fast-path CSs may run while B sits
between the two stores; `enrollment_window` runs 100 of them. A gate that reads
B's increment fails, as does every later gate until B is served: the ticket
stays visible until drained, then B's node is in the PQ. Fast-path CSs hold the
combiner lock, so once the increment is visible (on x86, when the locked
`fetch_add` completes) at most one fast-path CS, already past its gate,
finishes ahead of B.

Bound: one fast-path CS of delay per newcomer, counted from publication, not
from `active=true`. B's progress does not depend on the holder: B retries
`try_lock` every 8 backoffs, and its own combine drains its own entry. No
unlocker-side ring recheck is added. The fairness analysis needs a note, not a
new theorem.

The order `active=true` before publication is kept. With publication first, a
combiner could drain, serve and deactivate the node before the owner's
`active=true` lands. The node would then read active while in neither the ring
nor the PQ, so the owner's next `push_if_unactive` would skip enrollment and
that request would never be served.

## Risks

- Missed pending request (D2 wrong) breaks the PQ ordering: covered by the
  stress and window tests under Implementation.
- Interaction with E0(a) spin-then-park (agent in `../Locks-e0`): a fast-path
  holder unlocks without combining or waking. Today's spin-only waiters are
  immune (they retry `try_lock` every 8 backoffs), so this is not a fast-path
  requirement; but if E0(a) parks waiters, its no-stall rule must cover a
  waiter that enrolled after the holder's emptiness check and then parked
  (unlocker re-checks the ring after unlock, or waiters `try_lock` right
  before parking). Land after E0(a) and rebase.

  Merge contract with E0(a) (`spin_park`, change c9e5a6af in `../Locks-e0`):
  1. Every unlock path goes through `release_combiner()`, the non-combining
     fast-path holder and the gate-miss unlock included. On main this helper
     is just `unlock()`; under `spin_park` it re-checks `parked` after the
     unlock and combines, so the merge touches one hunk.
  2. Enrollment becomes a SeqCst CAS on `active`, and a retiring combiner may
     re-enroll a node, still under the lock. The gate's Acquire load of
     `!node.active` under the lock stays exact. Its SAFETY comment ("only this
     owner sets `active`") must then say "the owner, or a lock holder".
  3. `combiner_stat` must tolerate a combiner without a node. The fast path
     always has one.

## Evaluation

- Microbench with 1 thread, FC vs FC-PQ, >= 5 trials: target ratio >= 0.95.
- 2/4/8 threads with large non-CS (low contention): throughput and fast-path
  hit rate (temporary counter).
- 32 threads, zero non-CS: no regression vs current FC-PQ.
- Fairness: 1:8 cost heterogeneity service Jain at 8 and 32 threads unchanged
  within noise; redb 1/64 Jain (0.992 before).
- `cargo test --release`, plus the new stress test above.

## Implementation (2026-09-27, workspace `e0b-fastpath`)

Cargo features of `crates/libdlock`, all default-off; the names are the
benchmark contract:

- `fcpq_cached_tid`: D6 only.
- `fcpq_fast_path`: D1 + D2 + D4.
- `fcpq_fast_path_notime`: the same bypass without timestamps or usage charge,
  to isolate timestamp cost (fairness-incorrect, ablation only).
  `compile_error!` together with `fcpq_fast_path`.
- `fcpq_fast_path_stat`: per-node hit counter.
  `FCPQ::get_fast_path_hits() -> Option<u64>` reads the calling thread's
  count; `FCPQ::fast_path_hits() -> u64` (needs `I: Sync`) sums all threads.
  It reads 0 without a fast-path feature.

No features means the previous FC-PQ. The baseline changes only by a
`cfg(test)` hook in `push_node` and the `release_combiner()` refactor (I6).

Decisions refined while implementing:

- I0. Publish only after the gate fails, as D1 says. The draft sketch
  published first (`write data; complete=false`). The node can still be
  enrolled from the previous request, active in some combiner's PQ. That
  combiner serves the just-published request, pops the node again, and
  deactivates it before the owner's `try_lock`. The gate then passes and runs
  the request a second time. `phased_stress` caught this with the draft order
  and `fcpq_fast_path`: 4 of 28 `cargo test --release --lib` runs failed,
  and 0 of 20 runs of the new tests alone. Per-request counts then reported
  e.g. "request (4, 9107) executed 2 times". The fast path now runs the
  caller's `data` directly and never touches the payload slot or `complete`.
- I1. The gate runs once per request, before publication; the draft sketch
  gated in every retry. After publishing, a combiner can serve the request
  and deactivate the node between the waiter's last `complete` check and its
  next `try_lock`. A gate there would pass and run the request twice.
- I2. Never enroll while holding the combiner lock. The draft's
  `push_if_unactive(node); combine()` under the lock deadlocks once the
  64-slot ring is full (more than 64 enrolled threads), because the push spins
  for a drain only the holder can do. On a gate miss the holder combines if
  its node is still enrolled, where the push is a no-op. Otherwise it unlocks
  and runs the unchanged slow path.
- I3. No added CAS: the first `try_lock` result is handed to the slow-path
  loop's first iteration.
- I4 (D4 addendum). The fast path also applies the ring drain's newcomer
  initialization (usage 0 and served > 0 gives the running average). A thread
  whose first request takes the fast path is therefore charged what the slow
  path would charge. With `combiner_stat`, the CS time is added to
  `combiner_time_stat`.
- I5. The hit counter is an `AtomicU64` per node with a single writer
  (Relaxed load + store), so the cross-thread sum is race-free.
- I6. Every unlock, the baseline loop's included, goes through
  `release_combiner()`, the E0(a) merge shape (see Risks). This is a pure
  refactor for the baseline.

Tests, run in every build (`cargo test -p libdlock --release --lib`):

- `dlock2_unit_test::fc_pq_fast_path::{btree,bheap}::phased_stress`: 8
  workers, 8 rounds of three phases: one worker alone, all at once, then
  worker 0 hammering while the others trickle in. On every enrollment, odd
  workers stall 2000 spins between `active=true` and publication (a
  `cfg(test)` hook). The test checks that each response returns to its
  issuer, that tickets 1..=N each appear exactly once, and the per-worker
  execution counts. With `fcpq_fast_path_stat` it also checks solo-phase
  hits >= SOLO-1 and that the per-thread sum equals `fast_path_hits()`.
- `...::enrollment_window` (deterministic): worker 1 is held between
  `active=true` and publication while worker 0 issues 100 requests. With a
  fast path, none of them enroll. All precede worker 1, which is then served
  exactly once by its own retry.
- `...::single_thread_hits` (`fcpq_fast_path_stat`): with one thread, hits
  equal requests, the first request included. A fresh node is inactive, so it
  never enrolls.
- `dlock2::fc_pq::lock::accounting_tests`: with one thread, usage equals
  `total_usage` and served equals requests. A newcomer's usage equals the
  running average plus its CS, identical to the slow path. notime charges
  nothing.

Verification (2026-09-27, shared host; `uptime` load averages 4-80):

- `cargo build --release` and `cargo test --release`, with no features and
  with `fcpq_cached_tid`, `fcpq_fast_path`, `fcpq_fast_path,fcpq_cached_tid`,
  `fcpq_fast_path_notime` and `fcpq_fast_path,fcpq_fast_path_stat`: all
  pass. That is 155 lib tests plus 1 doctest, or 157 with stat, with 3
  ignored. All pass again with `--no-default-features`, i.e. without
  `combiner_stat` (exit 0 under pipefail).
- `cargo test --release --features F --lib -- fc_pq_fast_path
  accounting_tests`, 5 consecutive runs per combination: 30/30 pass.
- After I0, `cargo test --release --features fcpq_fast_path --lib` passed
  39 of 40 runs. The one failure was `unit_test::fc_fair_ban_slice_test`, a
  DLock1 test with a 60 s watchdog that is unrelated to FC-PQ. It also failed
  once with the draft order, at load average up to 80.
- The ignored `admission_churn_stress::pq_*` test (64/65/128 workers, ring
  over capacity) passes with `fcpq_fast_path`. This exercises I2.
- `fcpq_fast_path,fcpq_fast_path_notime` fails with the `compile_error!`.
- BUILD.md does not document Miri, but rustup's nightly-2026-04-28 has it.
  `rustup run nightly cargo miri test -p libdlock --lib --features F --
  fc_pq_fast_path accounting_tests` passes for F = none,
  `fcpq_fast_path,fcpq_fast_path_stat` and
  `fcpq_fast_path_notime,fcpq_cached_tid` (6, 8 and 6 tests, at the scaled-down
  Miri sizes). Miri reported no UB or data race.
- All FC-PQ tests under Miri: `rustup run nightly cargo miri test -p libdlock
  --lib --features F -- fc_pq ownership::pq_` (2026-09-28 01:02-01:03).
  The first attempt showed the existing `fc_pq_{btree,bheap}::threads_*`
  counter tests exceeding their 60 s `panic_after` watchdog, which counts
  Miri's virtual time. Unmodified `main` timed out the same way. The shared
  `ITERATIONS` is now 50 under `cfg!(miri)`; release runs keep 1000. After
  that change all tests pass: 15 with no features, 17 with
  `fcpq_fast_path,fcpq_fast_path_stat`, and 15 with
  `fcpq_fast_path_notime,fcpq_cached_tid`. No UB or data race was reported.
  After this edit the six release combinations were rerun with pipefail and
  all passed. The exception was one `fcpq_cached_tid` run, where
  `dlock2_unit_test::fc_sl::threads_{4,8}` (FCSL, not FC-PQ) timed out at
  120 s. The next two runs of that combination passed (01:06, load 40-60).

## Thread-churn test (2026-09-28, change on top of the PR #48 head)

`dlock2_unit_test::fc_pq_fast_path::{btree,bheap}::thread_churn`, ported from
an audit throwaway, runs in every build. Two FC-PQ instances and up to 4
long-lived hammers alternate between the locks. Meanwhile 2000 waves of up to
6 short-lived threads send 1..=40 requests each to one lock and exit
immediately after the last response. After the waves, a fresh thread probes
each lock. The test checks per-call response identity and `executions == 1`,
per-lock per-worker execution counts, and per-lock tickets 1..=N (the probe
included). It spawns at most `available_parallelism` spinning threads (with
one CPU, no hammers) and is `#[serial_test::serial]`, so the two queues never
overlap. `State::seen` rows now grow on demand so that the hammers can run
unbounded.

- Coverage: a throwaway probe (made `local_node` visible and read the slot
  before the first request) showed that 11988 of 12000 churn threads per test
  got an exited thread's recycled node, with and without `fcpq_fast_path`.
  None found that node still active. A combiner deactivates a served node in
  the same pass, before the next thread spawns. The test therefore covers
  slot reuse of a retired node carrying usage. It does not cover reuse of a
  still-enrolled node, which would require a pass to end at the 64-pop limit
  or stall with that node buffered.
- Mutations, each reverted: calling the delegate twice in `run_fast_path`
  (`fcpq_fast_path`) failed with "executed 2 times". Skipping every 1000th
  combine execution (default features) failed with "executed 0 times".
- Runtime and repeat runs (`taskset -c 48-63`, `timeout 120`, load < 2): 20/20
  pass with default features and 20/20 with `fcpq_fast_path`, about 1.0 s of
  wall time per `cargo test ... thread_churn` (both queues). The full
  `cargo test -p libdlock --release --features fcpq_fast_path --lib` gave
  157 passed and 3 ignored in 10.05 s.
