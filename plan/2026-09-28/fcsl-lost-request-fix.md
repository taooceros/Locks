# FC-SL lost-request fix (2026-09-28)

Status: implemented and verified on `fix/fcsl-lost-request` (stacked on
`sync/main-2026-09-28`, PR #47). Approved as a follow-up from the lock audit.

## Goal

Fix the hang in `dlock2::fc_sl` (FC-SL: flat combining with a skip-list
priority queue) and remove the data races in its usage accounting, without
changing its scheduling behaviour.

## Bug

`combine()` serves an entry, then stores `active = false` **before**
`complete = true`. The owner can wake in that window (bounded backoff, then
`continue 'outer`), see `active == false` and enroll a second entry E2 for
the request that is about to complete. It then observes `complete == true`
and returns, leaving E2 queued with `active == true`.

- If E2 is popped after the owner's next `complete = false`, E2 serves the
  new request. That path works.
- If E2 is popped **before** that, the combiner sees `complete == true` and
  drops it without clearing `active`. The owner's next request sees
  `active == true`, never enrolls, and spins forever.

FC-PQ does not hang because its buffer drain clears `active` whenever it
drops a completed entry (`fc_pq/lock.rs`, `node.active.store_release(false)`).

## Change

`crates/libdlock/src/dlock2/fc_sl/lock.rs`, `fc_sl/node.rs`:

1. **Lost request.** The `complete == true` (stale-entry) branch now stores
   `node.active.store(false, Release)` before dropping the entry.
2. **Data races.** Three fields were shared as plain values:
   - `Node::usage` was a plain `u64`, written by combiners and read or
     written by the owner in `push_node`. It is now an `AtomicU64`.
   - `total_usage` and `total_served` were `SyncUnsafeCell<u64>`, written by
     the combiner and read without the lock in `push_node`. They are now
     `AtomicU64`.
   - All accesses are `Relaxed`. A combiner updates each field with a load
     followed by a store, not an RMW, so the generated code stays the same.
3. **Aliasing.** The owner and the combiners now use `&Node`, as FC-PQ does,
   instead of overlapping `&mut Node`s. With `usage` atomic this is needed for
   the atomics fix to count. The payload is written with
   `node.data.get().write(..)`. `combiner_time_stat` becomes a
   `SyncUnsafeCell<u64>`, as in FC-PQ's node.

## Why clearing `active` cannot drop a live enrollment

Invariant: *at most one queue entry exists per node, and while `active` is
true, an entry for the node is either queued or popped by a combiner that
will store `active = false` next.*

- The owner enrolls only after it reads `active == false` (Acquire), and it
  stores `active = true` before inserting. The only writers of
  `active = false` are combiners holding the combiner lock, and each writes
  it right after popping the node's single entry: either after serving it or
  in the new stale-drop branch. So after any `active = false`, the queue
  holds no entry for the node, and the owner's next enrollment creates the
  only one.
- In the stale-drop case, E2 set `active = true` and nothing has cleared it
  since. So while the combiner holds E2, the owner cannot enroll a new entry.
  That makes E2 the node's only entry, and it is stale. Clearing `active`
  therefore cannot orphan a queued live entry.
- If the owner has started a new request and already saw `active == true`
  (no enrollment) before the clear, it has no entry and `active` is now
  false. Every `'outer` iteration calls `push_if_unactive`, and the spin loop
  is bounded (`Backoff::is_completed`), so the owner enrolls on its next
  iteration. That costs a delay, not a lost request.
- If the combiner instead reads the owner's new `complete == false`
  (Acquire, which pairs with the owner's Release after the payload write),
  it serves the new request from E2. This was already correct.
- Each request is served at most once. Serving stores `complete = true`
  before the combiner lock is released, so any later pop of an entry for the
  same request takes the drop branch.

FC-PQ restores the same property the same way. It never drops a completed
entry without clearing `active`. Before this fix, FC-SL's stale-drop branch
was the only path that dropped an entry and left `active` set.

An alternative is to store `complete = true` before `active = false` in the
serve path. That removes the window, but the owner could then see a stale
`active == true` at its next request and would depend on the combiner's
later clear plus its own re-check. The branch fix is smaller and mirrors
FC-PQ.

## Ordering justification for the atomics

- **`node.usage`.** The owner accesses it only after an Acquire load of
  `active == false`. Every combiner write to it comes before that combiner's
  Release store of `active = false`, and combiners are ordered by the
  combiner lock. So the owner sees the latest value. The owner's newcomer
  write happens before the skip-list insert, which publishes the entry the
  combiner pops. `Relaxed` is sufficient.
- **Totals.** Only the combiner-lock holder writes them, so a load followed
  by a store is an exact increment. The owner's lock-free read only feeds
  the newcomer-average heuristic, and the previous code read the same values
  in the same order. `Relaxed` is sufficient.

## Risks

- A small throughput change could come from the extra `active` store in the
  stale branch, which fires rarely. Atomics with `Relaxed` compile to plain
  `mov` on x86-64, so they should not cost anything. No benchmark has been
  run.

## Evaluation

All runs were pinned with `taskset -c 48-63` (socket 1, away from the
reserved CPUs 16-23), with a 30 s `timeout` per run of
`cargo test -p libdlock --release --lib dlock2_unit_test::fc_sl`:

- Unpatched: 2 of 12 runs hung, both in `fc_sl::threads_8` (runs 5 and 12).
  The audit had recorded 5 of 12.
- Patched: 50 of 50 runs passed, and an extra 300 of 300 passed. Each run
  took about 0.1 s.
- Full `cargo test -p libdlock --release --lib` (pinned to 32-47): 149
  passed, 0 failed, 3 ignored.
- `cargo build -p libdlock --release --no-default-features` builds, which
  covers the code without `combiner_stat`. `cargo fmt --all --check` is
  clean.
