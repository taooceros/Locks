# FC-PQ pass-length (H) ablation in redb (2026-09-29)

Question: is **combiner tenure**, FC-PQ's per-pass cap of H = 64 pops, what
makes FC-PQ beat FC in the redb write path (`all1`, None, 8 clients: 29.4k vs
27.1k tx/s in the closure-API rerun, HITM loads/tx 9 vs 22)? Plan:
[`plan/2026-09-29/fcpq-pass-length-ablation.md`](../../../plan/2026-09-29/fcpq-pass-length-ablation.md).
Report: [`report.typ`](report.typ) (HTML-first Typst). Tables: [`tables.md`](tables.md)
(every number below, median [min, max] of 3, plus per-worker tables and derived
ratios). Data: [`medians.json`](medians.json), [`ranges.json`](ranges.json),
[`summary.png`](summary.png); both come from
[`make_report_data.py`](make_report_data.py).

**Verdict.** Held for the uniform cohort, partly held for the mixed cohort.

- `all1`, 8 clients: capping H at the number of active nodes (`fc_pq_hn`) or at 8
  (`fc_pq_h8`) removes FC-PQ's whole tx/s advantage over FC (26.8k → 24.6k /
  24.5k tx/s; FC 24.8k), doubles HITM loads/tx (10.0 → 21.4 / 20.8; FC 20.5), cuts
  bodies per pass from 43.5 to 7.8 (FC 7.9), raises combiner changes per 1,000
  bodies from 9.4 to 51 / 56 (FC 54), and keeps service Jain ≥ 0.998. Every
  condition fixed in the plan holds.
- `half1_half64`, 8 clients: the locality metrics move exactly as in `all1` (HITM 11.5
  → 29.2 = FC's 29.2; bodies per pass 41.1 → 6.3; changes per 1k 8.8 → 64-66), but
  tx/s does **not** fall: 15.6k (H64), 15.8k (`hn`), 15.7k (`h8`) against FC 12.9k.
  The variants keep 106-109 % of FC-PQ's lead over FC (round 1: 84-86 %); the
  plan's "closing at least half of the gap" does not happen. FC-PQ's tx/s lead in this cohort
  is therefore not a tenure effect; service Jain is 1.000 for both capped
  variants (0.956 at H64; FC 0.853), and records/s fall (381k → 307k; FC 421k), so
  what remains is the serving order (equal service time shifts the mix toward the
  cheap 1-record transactions) [INFERENCE: the ordering explanation follows from
  Jain and records/s; no order-only ablation was run].
- Tenure has a tail-latency price that throughput hides. `all1` p99 response is
  2.62 ms at H64 against 0.66 (FC) and 0.72 ms (`hn`, `h8`), although the median
  response is lower (0.147 vs 0.328 ms per worker). `half1_half64` p99: 6.3 ms vs
  1.3 ms.

## 8-client table (round 2, final build; durability None; median [min, max])

### all1

| variant | tx/s | HITM loads/tx | L2 misses/tx | IPC | bodies/pass | combiner changes /1k bodies | service Jain | p99 ms | p99 worst worker |
|---|---|---|---|---|---|---|---|---|---|
| fc | 24830 [24796, 24876] | 20.5 [19.5, 20.9] | 283 | 0.25 | 7.94 [7.93, 7.95] | 54.4 [54.1, 56.2] | 0.997 [0.997, 0.998] | 0.655 | 0.721 |
| **fc_pq** (H=64) | 26811 [26488, 26858] | 10.0 [9.9, 10.0] | 165 | 0.27 | 43.54 [43.45, 43.89] | 9.4 [9.2, 9.6] | 0.998 [0.997, 1.000] | 2.621 | 2.884 |
| fc_pq_hn (H=active) | 24622 [24372, 24682] | 21.4 [21.1, 21.5] | 296 | 0.25 | 7.83 [7.80, 7.83] | 51.0 [47.8, 53.6] | 1.000 [1.000, 1.000] | 0.721 | 0.721 |
| fc_pq_h8 (H=8) | 24528 [24356, 24666] | 20.8 [20.6, 21.8] | 291 | 0.25 | 7.83 [7.82, 7.83] | 55.9 [55.3, 55.9] | 1.000 [1.000, 1.000] | 0.721 | 0.721 |
| upstream_gate (control) | 26742 [26660, 26866] | not run | | | | | 0.137 [0.125, 0.235] | 0.045 | 0.045 |

### half1_half64 (4 clients write 1 record, 4 write 64)

| variant | tx/s | records/s | HITM loads/tx | L2 misses/tx | IPC | bodies/pass | changes /1k | service Jain | p99 ms | p99 worst worker |
|---|---|---|---|---|---|---|---|---|---|---|
| fc | 12874 [12870, 13059] | 421020 | 29.2 [27.4, 29.6] | 506 | 0.28 | 7.94 | 59.7 [56.8, 60.1] | 0.853 [0.840, 0.861] | 1.311 | 1.311 |
| **fc_pq** (H=64) | 15582 [15236, 16080] | 381171 | 11.5 [11.4, 11.9] | 285 | 0.29 | 41.13 [41.10, 41.67] | 8.8 [8.6, 8.9] | 0.956 [0.944, 0.977] | 6.291 | 9.437 |
| fc_pq_hn | 15826 [15628, 15927] | 307232 | 29.2 [27.6, 29.5] | 456 | 0.27 | 6.32 [6.32, 6.33] | 63.6 [62.9, 63.7] | 1.000 [1.000, 1.000] | 1.311 | 1.573 |
| fc_pq_h8 | 15740 [15705, 15793] | 307272 | 29.2 [28.4, 29.3] | 454 | 0.27 | 6.31 [6.28, 6.38] | 65.5 [60.7, 68.5] | 1.000 [1.000, 1.000] | 1.311 | 1.573 |
| upstream_gate (control) | 20043 [19654, 26578] | 274028 | not run | | | | | 0.337 [0.243, 0.381] | 0.115 | 0.123 |

`upstream_gate` (redb's Mutex/Condvar) lets one client monopolise the writer
(service Jain 0.14 and 0.34), so its tx/s in `half1_half64` measures which request
size won (repeat range 19.7-26.6k), not a lock.

The per-client-count tables (1/2/4/8) for tx/s, service Jain, p99, bodies per
pass, combiner changes and combiner concentration are in `tables.md`. The
1-client cells are a build control: all five variants run the same fast path and
are within 0.7 % (`all1`: 27.7-27.9k tx/s).

Per-pass change probability is about the same for every variant (changes per pass
= changes per 1k bodies × bodies per pass / 1000 = 0.41 for H64, 0.43 for FC, 0.40
for `hn`, 0.44 for `h8`, `all1`, 8 clients): the pass length alone sets how often
the combiner (and with it the B-tree working set) moves.

## Per-worker response time and combiner service (8 clients)

All per-worker p50/p99 values, and the share of all bodies each worker ran while it
was the combiner (its own requests included, and separately those for other workers),
are in `tables.md` ("Per-worker ..."). Readings:

- `all1`: FC-PQ's median response is 0.123-0.164 ms for every worker and its p99 2.62-2.88 ms for
  every worker (no worker is singled out); FC, `hn` and `h8` are 0.328 ms / 0.49-0.72 ms.
- The combiner role is spread over all workers (largest single-worker share of all bodies at 8
  clients: `all1` 0.15-0.16 for every variant, 1/8 = 0.125; `half1_half64` 0.18-0.24, highest under
  H64). Under H64 the combiner almost never serves its
  own request (1.1 % of `all1` requests run on their requester's thread; 11.9 % for FC, `hn`, `h8`),
  i.e. a worker becomes combiner after its own request was already served and then runs about
  44 bodies for others [INFERENCE: read from the two fractions, not traced].
- `half1_half64`: under H64 each 64-record worker runs 15-22 % of the bodies while combiner against
  5-9 % for each 1-record worker; FC-PQ's p99 is 5.2-5.8 ms (1-record workers) and 6.8-9.4 ms
  (64-record workers); `hn` gives 0.79-0.85 ms and 1.44-1.57 ms.

## Counter overhead (stats build)

`combiner_pass_stat` adds a few plain counters inside each combining pass, so it lives
in a separate binary (`redb-patched_stats`). Its tx/s over the primary build, all
client counts: `all1` 0.999-1.015; `half1_half64` 0.983-1.027 (round 1: 0.946-0.992,
the opposite sign). The stats roots ran after the timed roots, so the two rounds
cannot separate a small counter cost from drift; the cost is not resolved at about
±3 %, and the timed results come from the build without counters.

## Default unchanged (H = 64)

Interleaved A/B, `fc_pq`, None, 8 clients, CPUs 16-23, 8 pairs of 2 s runs, previous
closure-API build (`redb-build-06`, no `PassCap`, no combiner counters) vs the final build,
paired ratio new/old: `all1` median 0.999 [0.991, 1.007]; `half1_half64` 1.016 [0.957,
1.040]. The earlier build of the same sources (round 1): 1.000 [0.974, 1.005] and 0.989
[0.975, 1.028]. No difference is resolved; earlier FC-PQ results stay valid.

## What was run

| Stage | Root | Cells | Failed |
|---|---|---:|---:|
| Build (final) | `.worktree/redb-build-h02` | 4 binaries | - |
| Correctness gate (final build) | `.worktree/redb-build-h02/correctness-01/correctness.json` | 145 cases | 0 |
| Timed (round 2) | `.worktree/fcpqh-timed-02` | 120 | 0 |
| Stats build (round 2) | `.worktree/fcpqh-stats-02` | 96 | 0 |
| Perf, 8 clients (round 2) | `.worktree/fcpqh-perf-02` | 24 | 0 (every event 100 % running) |
| Round 1 (earlier build `redb-build-h01`; same matrix) | `.worktree/fcpqh-{timed,stats,perf}-01` | 120 + 96 + 24 | 0 |

Matrix: variants fc, fc_pq, fc_pq_hn, fc_pq_h8 and upstream_gate (the stats and perf roots
have the four delegation variants), cohorts `all1` and `half1_half64`, durability None,
clients 1/2/4/8 (perf: 8), 3 repetitions (seeds as the rerun), variant order shuffled
per cell, fresh process per 2 s cell, CPUs 16-23, NUMA node 0, power setup S1 at 3.0 GHz,
every command under an exclusive `MEASUREMENT_LOCK`. Off-target clock (outside 3.0 GHz ± 2 %):
timed round 2 6/120 (2.92-2.94 GHz; `all1` c4 upstream_gate, c8 upstream_gate, c8 fc_pq,
2 × c8 fc_pq_h8; `half1_half64` c8 fc_pq_hn), stats 0/96, perf 0/24; round 1: 0 everywhere. The
flagged cells are group minima except one: in `all1` c8 `fc_pq_h8` the median (24528) is a flagged
cell too (2.94 GHz); the other off-target clock cells are also within 3 % of target.

Provenance (final build): crate SHA-256 `ae323eb0…6a06` (as the rerun), patched tree
`9e57be76fba0ad56bbb8e9f0c0679ef46831de84ef1381204200b3e368306366`, patches
unchanged by this change (`0001` `639675b3…`, `0002` `ae60b60d…`; `0002` differs
from the closure-rerun's `590f2cd3…` because the base commit `610dc3be` renamed the control
variants). Binaries: `redb-patched`
`884207c2…e621f9`, `redb-patched_stats` `862968cd…eae0ff`, `redb-test_hooks`
`6733ad3b…ede130`, `redb-upstream` `80f5d95e…395794`; rustc 1.100.0-nightly
(6bb1652a0 2026-09-22). Run manifests (SHA-256): timed `c66758be…305393`, stats
`548872c0…ff09802`, perf `3e676d99…cba88`. `build.json`'s `git_head` names the
default workspace's repository, not this jj workspace (known since the rerun).

## Gate

Final build: 145 cases (9 variants), **145 passed**. The earlier build `redb-build-h01`
(before the `executed_bodies` counter and the locked `pass_stats`): 145 cases, 144 passed;
the single failure was `std_mutex` / `test_hooks` / `transfer-immediate`, `redb write trial
failed: integrity check after reopen had to repair the database` - the known intermittent
transfer-immediate integrity repair, accepted and not rerun; all 17 cases of each new variant
(`fc_pq_hn`, `fc_pq_h8`) passed both times, including the lone-requester fast-path case and the
combiner-evidence cases. The stats binary differs only by counters and was not gated; every
timed cell of its root verifies exact contents after close/reopen, and the trial checks
that the lock's bodies equal the committed transactions and no pass exceeded its cap.

libdlock unit tests (default features and with `fcpq_fast_path,fcpq_fast_path_stat,
combiner_pass_stat`): 24 and 26 tests in the FC-PQ/pass filters passed, including
`pass_cap_tests` (a pass never serves more than a fixed cap 1-5 while more requests are
pending, serves all when the cap covers them, `Active` = the enrolled nodes at pass
start, contended stress for caps 1, 2, 8, 64 and `Active`, cap 0 rejected, snapshot taken
while combining).

## Mechanism choice

`PassCap` is a runtime field of `FCPQ` (`FCPQ::new` = `Fixed(64)`, `with_pass_cap`), read
once per pass, not a const generic. A const generic changes `FCPQ`'s type at every use
(aliases in `dlock2.rs`, the UpScaleDB and redb bridges, the benchmark dispatch), builds
one monomorphization per policy, and cannot express `Active` (it depends on the queue
at pass start). The field costs one read per pass against a body of tens of µs and lets
one binary host all variants. The A/B above bounds the default path's cost.

## Caveats and deviations

- Three repetitions; ranges are not confidence intervals. Two full rounds agree on
  every `all1` number within 1-3 % and on every mechanism metric. The mixed cohort's
  FC-PQ repeat range is 5.4 % (15.2-16.1k), so differences of ±3 % between H64 and the
  capped variants are not resolved; only "no tx/s gain from the long tenure" is.
- `combiner changes` compares each pass's combiner (the thread's ThreadLocal node address)
  with the previous pass's, empty passes included; `combiner_changes_nonempty_per_1k_bodies`
  (in `summary.json`) compares only passes that ran a body. A fast-path request counts as a
  one-body pass. Bodies per pass is a ratio of totals, and the histogram is in the raw rows.
- Requests-served-while-combiner is counted per worker thread (`executed_bodies`, own
  requests included; `served_for_others` without them), for the whole run including requests
  draining after the 2 s window, from two thread-local increments per body (always on; all
  variants alike). The harness did not time combiner passes, so there is no pass-duration
  distribution; bodies per pass × ≈ 35-40 µs per `all1` body estimates it (H64: about 1.6 ms).
- p50/p99 are bucket upper bounds of a histogram with 8 sub-buckets per octave (12.5 %
  resolution), of the requests completed inside the 2 s window; per-worker values are
  medians over the 3 repeats. A worker that completed nothing (upstream_gate's starved
  clients) has none.
- `Active` counts the entries in the queue at pass start, and a pop of a completed node
  spends part of the cap without running a body: passes served 7.8 `all1` and 6.3
  `half1_half64` bodies per pass at 8 clients, not 8. At 2 clients `hn` serves exactly 1 body per
  pass in `half1_half64` (the cap is 1 when one node is enrolled); that cell is not explained here.
- The FC-PQ fast path is on in every FC-PQ variant and is the only path at 1 client; at 2
  clients it still serves some requests (one-body passes). perf counters are process totals over all
  threads (spinning waiters included), user mode only.
- This isolates tenure within FC-PQ. It does not attribute FC-PQ's advantage over FC to
  the announcement ring, node layout or the priority order; in `half1_half64` the remaining lead
  is consistent with the ordering, but that was not isolated.
- Not run, as assigned: Immediate durability, `half1_half8`, `transfer`, the upstream redb test
  suite (the patches are unchanged), clock setups other than S1. The plan's "wait for approval"
  was met by the assignment.
