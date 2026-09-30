# FC-PQ pass-length (H) ablation in redb

Status: approved by the assignment (workspace `fcpq-h-ablation`,
`.worktree/jj/fcpq-h-ablation`, based on `evidence/redb-closure-rerun`); outcome
recorded in [the evidence report](../../docs/evidence/fcpq-pass-length-2026-09-29/RESULTS.md).

## Goal

Explain why FC-PQ beats FC in the redb write path (29.4k vs 27.1k tx/s,
`all1`, None, 8 clients; HITM/tx 9 vs 22; same instructions/tx; lock
utilization ≈ 98 % for both) despite doing more work per request. Test one
mechanism: **combiner tenure**.

## Hypothesis

FC-PQ's `combine()` (`crates/libdlock/src/dlock2/fc_pq/lock.rs`, `const H: usize
= 64`) pops up to 64 queue entries per pass and re-inserts every served node, so
a node whose owner resubmits during the pass is served again in the same pass.
One combiner therefore runs many bodies in a row. FC (`fc/lock.rs`, `combine`)
serves each active node at most once per pass and then unlocks, so its combiner
changes roughly once per pass of ≤ c bodies.

Prediction: capping H at the number of active nodes pushes FC-PQ toward FC's
throughput and HITM rate, keeps fairness, and lowers the combiner's own latency
(a pass of up to 64 bodies ≈ 2 ms at ≈ 30 µs/body). Longer tenure keeps the
protected B-tree pages in one core's cache, so fewer cross-core transfers per
transaction. If tenure were the cause, FC-PQ with H = active should lose its
advantage over FC; if the advantage survives H = active or H = 8, tenure is not
the mechanism (priority order, fast path, or waiter behaviour would be next).

Falsifiers, stated before measuring:

- **Held:** at 8 clients `all1`, fc_pq_hn and fc_pq_h8 tx/s fall toward FC
  (closing ≥ half of the fc_pq − fc gap), HITM/tx rises toward FC's, bodies per
  pass and changes per 1k bodies move toward FC's, and service Jain stays ≥ 0.99
  (mixed cohort: not worse than H64 by > 0.02).
- **Not held:** fc_pq_hn/h8 tx/s within 2 % of fc_pq with unchanged HITM/tx.
- **Partly held:** anything between.

## Variants

| Variant | Per-pass pop cap |
|---|---|
| `fc_pq` | 64 (unchanged default; existing results stay valid) |
| `fc_pq_hn` | `active`: entries in the priority queue at pass start (after draining the announcement ring) |
| `fc_pq_h8` | 8 (= the number of clients at the largest point) |
| `fc` | reference: one pass serves each active node once |
| `upstream_gate` | control: redb's own Mutex/Condvar gate (no delegation) |

The FC-PQ fast path stays on for all three FC-PQ variants (at 1 client every
variant is the same code path; 1-client cells are a build control).

## Mechanism

A runtime field `pass_cap: PassCap` (`Fixed(n)` or `Active`), set by
`FCPQ::with_pass_cap`; `FCPQ::new` = `Fixed(64)`. Reasons: one binary hosts all
variants (identical code layout, randomised order inside one process image);
the cost is one load of a read-only field per pass, against a ≈ 30 µs body; a
const generic would change `FCPQ`'s type at every instantiation (crate aliases,
CLI dispatch, redb bridge) and cannot express `active` anyway. The default path
is checked against the previous build with interleaved A/B pairs.

## Measurement

Host and method as the rerun (CPUs 16-23, NUMA node 0, power setup S1 at
3.0 GHz, `MEASUREMENT_LOCK`, fresh process per 2 s cell).

1. **Gate** (correctness.py) for the new variants; the known intermittent
   transfer-immediate integrity repair is acceptable.
2. **Timed:** fc, fc_pq, fc_pq_hn, fc_pq_h8 and upstream_gate; cohorts `all1`
   and `half1_half64`; durability None; 1/2/4/8 clients; 3 repeats; variant
   order shuffled per cell.
3. **Perf cohort:** the four delegation variants, same two cohorts, 8 clients:
   HITM, L2/LLC misses, IPC.
4. **Stats build:** separate binary with `combiner_pass_stat`; same timed matrix
   over the four delegation variants: bodies per pass and combiner changes per
   1,000 bodies. Its tx/s against the primary matrix measures the counters'
   overhead.

Metrics: tx/s, records/s, service Jain and long-client service share, per-worker
p50/p99 response time (8 sub-buckets per octave, upper bounds), bodies served by
a worker for other clients while it was combiner (`served_for_others`), HITM
loads/tx, L2/LLC misses/tx, IPC, bodies per pass (ratio of totals, empty passes
included; histogram), combiner changes per 1,000 bodies.

## Risks

- **Clock drift.** Cells off 3.0 GHz are flagged; conclusions use medians of
  three and the flag counts are reported.
- **Three repeats** are not confidence intervals; effects below the repeat range
  are not claimed.
- **`active` wastes pops** on complete-but-enrolled nodes (they count against the
  cap), so passes may serve fewer than `active` bodies. The report states
  measured bodies per pass rather than the cap.
- **Stats counters** run only inside a separate build; its throughput is not a
  primary result.
- **Attribution.** This isolates tenure within FC-PQ. It does not attribute
  FC-PQ's advantage over FC to any other difference (priority order, the
  announcement ring, node layout).
- **Combiner identity** is the combiner's node address, so a thread that exits
  and whose slot is recycled looks like the same combiner (not an issue here:
  threads live for the whole cell).
- Harness change adds one `usize` to the lock request for all variants; the
  default-variant A/B check bounds its cost.

## Deliverables

libdlock `PassCap` + tests + `combiner_pass_stat`; redb variants wired through
build.py, correctness.py, run.py and the README; evidence under
`docs/evidence/fcpq-pass-length-2026-09-29/` (RESULTS.md, report.typ, medians
JSON, figure); docs/evidence/README.md row; TODO.md; PR from
`experiment/fcpq-pass-length` against main.

## Outcome (2026-09-30)

Implemented and measured as planned (two full rounds; the second on the final build).
Verdict: **held** for `all1` (every stated condition met: tx/s 26.8k → 24.6k / 24.5k vs FC
24.8k, HITM/tx 10.0 → 21.4 / 20.8 vs FC 20.5, bodies per pass 43.5 → 7.8, combiner
changes per 1k bodies 9.4 → 51 / 56, service Jain ≥ 0.998); **partly held** for
`half1_half64` (tenure metrics move as predicted but tx/s does not fall: 15.6k → 15.8k /
15.7k vs FC 12.9k). H = 64 costs tail latency (p99 2.6 vs 0.7 ms in `all1`). Changes
against the plan: the per-worker combiner attribution counts all bodies the worker's thread ran
(`executed_bodies`) next to those for others; the public `pass_stats()` snapshot takes the
combiner lock; the new variants are opt-in (`--variants`) so the default matrix is unchanged; a second
round was run because the harness gained the counters after round 1. See
[RESULTS.md](../../docs/evidence/fcpq-pass-length-2026-09-29/RESULTS.md).
