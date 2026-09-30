# redb rerun on the closure-API build (2026-09-29)

Rerun of the redb-internal experiment on merged `main` (PR #54, closure-style
delegated write API, thin LTO for every binary), following
`integration/redb/README.md` (Workflow). The earlier numbers were measured on
redb-internal's fixed-body **non-LTO** build (`redb-build-04`). The README
(Known limits, "Codegen parity") says the two builds are not comparable at the
few-percent level; this comparison is limited to that.

Raw data stays under `.worktree/` in the `redb-rerun` jj workspace and is not
copied here:

| Stage | Root | Cells | Failed |
|---|---|---:|---:|
| Build | `.worktree/redb-build-06/build.json` | — | — |
| Correctness gate | `.worktree/redb-build-06/correctness-01/correctness.json` | 111 cases | 1 (known intermittent, below) |
| Smoke | `.worktree/redb-rerun-06/smoke`, `analysis-smoke/summary.{json,md}` | 504 | 0 |
| Formal matrix (insert) | `.worktree/redb-rerun-06/runs`, `analysis/summary.{json,md}` | 504 | 0 |
| Perf cohort | `.worktree/redb-rerun-06/perf`, `analysis-perf/summary.{json,md}` | 168 | 0 (every event 100 % running) |

Old roots used for the comparison (in `.worktree/jj/redb-internal/.worktree/`):
`redb-smoke-01` (S0, 336 cells), `redb-formal-03-fixed3g` (S1 3.0 GHz, 360
cells, five variants: no `upstream_gate`/`std_mutex`), `redb-perf-03-fixed3g`
(S1 3.0 GHz, 168 cells). Their readings are in redb-internal's
`plan/2026-09-28/redb-internal-rerun.md` and `redb-perf-02-clock.md`.

## Build and provenance

- Source: jj commit `4a0ea155` (= `main@origin`, merge of PR #54), workspace
  `redb-rerun`. `python3 -m integration.redb.build --output-dir
  .worktree/redb-build-06 --jobs 8` under a shared `MEASUREMENT_LOCK`.
- **Caveat:** `build.json` `git_head` (`43c6cf9…`) and `git_dirty_paths` are
  **not** this source. The jj workspace is not colocated, so `git` walks up to
  the default workspace's repository and records that repository's HEAD and
  dirty files. The source hashes below were taken from the rerun workspace's
  files.
- rustc `1.100.0-nightly (6bb1652a0 2026-09-22)`, the same rustc as `redb-build-04`.
- Crate `redb-3.1.0.crate` SHA-256 `ae323eb086579a3769daa2c753bb96deb95993c534711e0dbe881b5192906a06`
- Upstream tree `a660770ef93cdf2604cf8c0565955f54859265502c42d8c01e635ec6aa20c792`
- Patched tree `a153400c3ca27f82e55f10809fe6c2841fd30e15b70ce82380cbcdbcbcd9d37a`
- `0001-delegated-writer-admission.patch` `639675b3dc580b7e3a609869ed1e53928c2aa3b7208605c7e24525cc855e99e6`
- `0002-closure-write-body.patch` `590f2cd3a4c03f7da9cbe4fc58d34cc600241bdba9dbe3aad23e8ac51eaadae0`
- Binaries: `redb-upstream` `7ada9a3737d068c7218a51ae3bff8e5d6c7e209a6dd709a3e3bd4e636db9dcda`,
  `redb-patched` `555824bc92b79985feb8130d0c6eebe6c6914c1d527e148dacdf2cf58b47163b`,
  `redb-test_hooks` `bfd3694cb73cc2f1c8724b78d6e5254bcbef3905fc84a2a21dc8a5c586c1f003`
  (instrumented; patched with `fcpq_fast_path`, `fcpq_fast_path_stat`).
- Run manifest `.worktree/redb-rerun-06/manifest.json` SHA-256
  `66bb31d05588726792cacfec41b2bbfb68781c2d232d61083c741cd929c735f3`.
- Placement: CPUs 16-23, NUMA node 0 (`membind: 0`), kernel 6.17.7, ext4, as in
  formal-03.
- Power: `--check-power` first. S1 at 3.0 GHz matches the host. S0 is refused
  (the policies are pinned `performance` 3.0/3.0 GHz), and S2 is refused (C6
  is enabled on 16-23). Prepared with `--power-setup S1 --fixed-ghz 3.0`, the
  same as formal-03/perf-03.
- Off-target clock cells (outside 3.0 GHz ± 2 %): smoke 112/471 decided (23.8 %),
  formal 70/475 (14.7 %), perf 18/168 (10.7 %). Old: formal-03 45/331 (13.6 %),
  perf-03 18/168 (10.7 %).

## Correctness gate

111 cases, 110 passed. The single failure is **mcs / `redb-patched` /
`transfer-immediate`**: `stderr` = `redb write trial failed: integrity check
after reopen had to repair the database` (exit 1,
`correctness-01/mcs-patched-transfer-immediate/record.json`). This is the known
intermittent failure (transfer-immediate on a delegated variant, integrity
check after reopen, ≈1 in 20). The same case passed for mcs on `test_hooks` and
for every other variant on both binaries. There were no other failures, so the
workflow continued. The gate was run once, as instructed, and not rerun.

### Upstream redb tests on the patched tree

`cargo test --release --lib --test basic_tests --test integration_tests
--test multithreading_tests` in a copy of `.worktree/redb-src/redb-3.1.0`
(`.worktree/redb-upstream-tests-06`). The only change to the copy is an empty
`[workspace]` table appended to its `Cargo.toml`, because the copy sits inside
the repository's Cargo workspace. Result: lib 37, basic_tests 69,
integration_tests 56 and multithreading_tests 3 passed, 0 failed.

## Headline tables (formal matrix, 504 insert cells)

Median of 3 repetitions (repeat range as % of the median). `None` is the
primary regime; `Immediate` is the fsync control. The full tables with min/max,
records/s, CPU-s, clock and tx/s @ref are in `analysis/summary.md`.

### tx/s, None

| cohort | c | upstream | upstream_gate | std_mutex | mcs | uscl | fc | fc_pq |
|---|---|---|---|---|---|---|---|---|
| all1 | 1 | 30332 (1%) | 30402 (1%) | 30503 (0%) | 30548 (1%) | 30538 (0%) | 30481 (1%) | 30654 (0%) |
| all1 | 2 | 29264 (1%) | 29244 (1%) | 29158 (1%) | 25546 (1%) | 29200 (1%) | 29600 (1%) | 29398 (1%) |
| all1 | 4 | 29273 (0%) | 29362 (0%) | 29416 (0%) | 24424 (1%) | 28719 (1%) | 28443 (0%) | 28600 (2%) |
| all1 | 8 | 29250 (0%) | 29438 (0%) | 29460 (0%) | 23910 (0%) | 27936 (0%) | 27096 (0%) | 29419 (0%) |
| half1_half8 | 1 | 26636 (1%) | 26590 (0%) | 26757 (0%) | 26686 (0%) | 26736 (0%) | 26718 (1%) | 26668 (0%) |
| half1_half8 | 2 | 28818 (21%) | 24438 (13%) | 26427 (20%) | 22322 (0%) | 25752 (0%) | 25835 (1%) | 25848 (1%) |
| half1_half8 | 4 | 25782 (14%) | 26390 (10%) | 25392 (15%) | 21434 (1%) | 25140 (0%) | 24642 (0%) | 25011 (4%) |
| half1_half8 | 8 | 29274 (18%) | 28106 (20%) | 27068 (12%) | 21146 (0%) | 24570 (1%) | 23601 (1%) | 25911 (0%) |
| half1_half64 | 1 | 15252 (0%) | 15254 (1%) | 15258 (0%) | 15276 (0%) | 15264 (2%) | 15177 (1%) | 15324 (1%) |
| half1_half64 | 2 | 23150 (72%) | 26648 (72%) | 10310 (184%) | 13828 (0%) | 18686 (1%) | 14962 (1%) | 16705 (1%) |
| half1_half64 | 4 | 10208 (8%) | 10256 (71%) | 11859 (65%) | 13290 (0%) | 18258 (2%) | 14200 (2%) | 18462 (5%) |
| half1_half64 | 8 | 29262 (66%) | 11976 (160%) | 14642 (70%) | 13023 (1%) | 17793 (0%) | 13869 (1%) | 16733 (4%) |

`half1_half64` records/s, None (the half cohorts' work metric):

| c | upstream | upstream_gate | std_mutex | mcs | uscl | fc | fc_pq |
|---|---|---|---|---|---|---|---|
| 2 | 212812 | 86718 | 648438 | 449410 | 345908 | 484910 | 421864 |
| 4 | 653312 | 655200 | 600752 | 431957 | 336376 | 465440 | 347794 |
| 8 | 29312 | 588205 | 534865 | 423248 | 323352 | 451234 | 412182 |

The Mutex/Condvar controls (upstream, upstream_gate, std_mutex) let one client
take the write path for long runs, so in the half cohorts their tx/s and
records/s depend on which request size wins (repeat ranges up to 184 %).

### tx/s, Immediate (control)

| cohort | c | upstream | upstream_gate | std_mutex | mcs | uscl | fc | fc_pq |
|---|---|---|---|---|---|---|---|---|
| all1 | 1 | 14543 (42%) | 14595 (1%) | 14566 (4%) | 14598 (38%) | 14575 (11%) | 14668 (8%) | 14610 (34%) |
| all1 | 2 | 14480 (0%) | 14406 (5%) | 14414 (1%) | 12606 (35%) | 14580 (2%) | 14029 (20%) | 14595 (1%) |
| all1 | 4 | 14426 (1%) | 14453 (3%) | 14548 (1%) | 13444 (10%) | 14257 (6%) | 14141 (6%) | 14365 (4%) |
| all1 | 8 | 14552 (1%) | 14470 (2%) | 14454 (1%) | 13408 (3%) | 14165 (4%) | 13980 (2%) | 14621 (2%) |
| half1_half8 | 1 | 13018 (3%) | 13093 (1%) | 13082 (0%) | 13054 (0%) | 13124 (2%) | 12968 (3%) | 13088 (1%) |
| half1_half8 | 2 | 13122 (17%) | 12904 (7%) | 12916 (7%) | 12165 (0%) | 13032 (1%) | 12821 (4%) | 12902 (3%) |
| half1_half8 | 4 | 13124 (21%) | 13516 (5%) | 13360 (5%) | 12062 (1%) | 12644 (2%) | 12471 (2%) | 12630 (3%) |
| half1_half8 | 8 | 13630 (4%) | 12977 (10%) | 12964 (7%) | 11740 (3%) | 12444 (2%) | 12278 (1%) | 12828 (0%) |
| half1_half64 | 1 | 8038 (1%) | 8224 (2%) | 8132 (1%) | 8126 (1%) | 8164 (1%) | 8102 (0%) | 8192 (2%) |
| half1_half64 | 2 | 10908 (45%) | 11465 (5%) | 12553 (14%) | 7647 (2%) | 9586 (1%) | 7990 (7%) | 8604 (16%) |
| half1_half64 | 4 | 6144 (47%) | 9162 (21%) | 6846 (38%) | 7624 (1%) | 9000 (27%) | 7812 (1%) | 8562 (2%) |
| half1_half64 | 8 | 8903 (44%) | 10251 (21%) | 7361 (18%) | 7381 (3%) | 8828 (29%) | 7702 (14%) | 7378 (31%) |

### Service-time fairness (formal, median)

`service_jain` (1-client cells are 1 by definition):

| durability | cohort | c | upstream | upstream_gate | std_mutex | mcs | uscl | fc | fc_pq |
|---|---|---|---|---|---|---|---|---|---|
| none | all1 | 2 | 0.500 | 0.986 | 0.830 | 1.000 | 1.000 | 1.000 | 1.000 |
| none | all1 | 4 | 0.545 | 0.501 | 0.762 | 1.000 | 1.000 | 0.998 | 0.994 |
| none | all1 | 8 | 0.299 | 0.274 | 0.232 | 1.000 | 1.000 | 0.997 | 0.999 |
| none | half1_half8 | 2 | 0.500 | 0.606 | 0.552 | 0.994 | 1.000 | 0.993 | 1.000 |
| none | half1_half8 | 4 | 0.570 | 0.492 | 0.519 | 0.994 | 1.000 | 0.994 | 0.991 |
| none | half1_half8 | 8 | 0.205 | 0.146 | 0.423 | 0.994 | 1.000 | 0.989 | 0.999 |
| none | half1_half64 | 2 | 0.607 | 0.500 | 0.502 | 0.864 | 1.000 | 0.844 | 0.953 |
| none | half1_half64 | 4 | 0.414 | 0.250 | 0.303 | 0.866 | 1.000 | 0.828 | 0.997 |
| none | half1_half64 | 8 | 0.213 | 0.287 | 0.189 | 0.869 | 1.000 | 0.834 | 0.948 |
| immediate | all1 | 8 | 0.250 | 0.269 | 0.296 | 1.000 | 1.000 | 0.999 | 0.999 |
| immediate | half1_half8 | 8 | 0.230 | 0.205 | 0.315 | 0.997 | 1.000 | 0.997 | 1.000 |
| immediate | half1_half64 | 2 | 0.841 | 0.573 | 0.695 | 0.887 | 1.000 | 0.880 | 0.975 |
| immediate | half1_half64 | 4 | 0.534 | 0.422 | 0.573 | 0.907 | 1.000 | 0.914 | 0.986 |
| immediate | half1_half64 | 8 | 0.250 | 0.273 | 0.217 | 0.917 | 1.000 | 0.906 | 0.992 |

`long_service_share` for `half1_half64`, None (0.5 = equal), c 2/4/8: MCS
0.699/0.697/0.694, FC 0.715/0.727/0.723, FC-PQ 0.611/0.510/0.611, U-SCL
0.500/0.504/0.504. `tx_jain` for `half1_half64`, None, c 2/4/8: U-SCL
0.834/0.835/0.828 and FC-PQ 0.950/0.842/0.925, against 1.000 for MCS and FC
(equal counts, unequal service). FC-PQ fast-path hit rate: 1.000 at 1 client,
0.163 (`all1`) / 0.222 (`half1_half64`) at 2, 0.000 at 4-8.

### upstream_gate_vs_upstream (formal)

| durability | cohort | c | ratio | noise | verdict |
|---|---|---|---|---|---|
| none | all1 | 1 | 1.002 | 0.010 | within noise |
| none | all1 | 2 | 0.999 | 0.011 | within noise |
| none | all1 | 4 | 1.003 | 0.005 | within noise |
| none | all1 | 8 | 1.006 | 0.002 | outside noise |
| none | half1_half8 | 1 | 0.998 | 0.005 | within noise |
| none | half1_half8 | 2 | 0.848 | 0.209 | within noise |
| none | half1_half8 | 4 | 1.024 | 0.143 | within noise |
| none | half1_half8 | 8 | 0.960 | 0.203 | within noise |
| none | half1_half64 | 1 | 1.000 | 0.011 | within noise |
| none | half1_half64 | 2 | 1.151 | 0.722 | within noise |
| none | half1_half64 | 4 | 1.005 | 0.712 | within noise |
| none | half1_half64 | 8 | 0.409 | 1.599 | within noise |
| immediate | all1 | 1 | 1.004 | 0.423 | within noise |
| immediate | all1 | 2 | 0.995 | 0.046 | within noise |
| immediate | all1 | 4 | 1.002 | 0.025 | within noise |
| immediate | all1 | 8 | 0.994 | 0.023 | within noise |
| immediate | half1_half8 | 1 | 1.006 | 0.029 | within noise |
| immediate | half1_half8 | 2 | 0.983 | 0.169 | within noise |
| immediate | half1_half8 | 4 | 1.030 | 0.211 | within noise |
| immediate | half1_half8 | 8 | 0.952 | 0.102 | within noise |
| immediate | half1_half64 | 1 | 1.023 | 0.019 | outside noise |
| immediate | half1_half64 | 2 | 1.051 | 0.455 | within noise |
| immediate | half1_half64 | 4 | 1.491 | 0.474 | outside noise |
| immediate | half1_half64 | 8 | 1.151 | 0.441 | within noise |

Every uncontended cell is within 0.998-1.006 except Immediate `half1_half64`
(1.023). The None `all1` 8-client "outside noise" is a +0.6 % gap against a
0.2 % spread. Of the uncontended cells, only in `all1` are both Mutex/Condvar
variants fair enough (both far from 1) that the ratio still measures build
overhead.

### Smoke (504 cells, incl. `transfer`)

The smoke's insert cells repeat the formal pattern: `all1` None c1 30.4-30.7k
tx/s for all seven variants, and the same fairness order. Its
`upstream_gate_vs_upstream` is 0.996-1.005 in every None `all1`/`transfer` cell and
in 1-client `half1_half64`. The one outside-noise value is None `half1_half64`
c4 (1.704 against a 0.593 range), a Mutex/Condvar winner effect. **Transfer**
(new, no old counterpart), committed tx/s, None, c 1/2/4/8:

| variant | 1 | 2 | 4 | 8 |
|---|---|---|---|---|
| upstream | 29738 | 28472 | 28446 | 28672 |
| mcs | 30078 | 24508 | 23174 | 22488 |
| uscl | 29873 | 28502 | 27660 | 26964 |
| fc | 29829 | 29018 | 27630 | 26569 |
| fc_pq | 29882 | 28912 | 27324 | 28024 |

Transfer `service_jain` is ≥ 0.997 for MCS/U-SCL/FC/FC-PQ at every count, and
0.36-0.37 (upstream/upstream_gate) and 0.60 (std_mutex) at 8 clients. Aborted
transfers are not charged service (README, Known limits).

### Perf cohort (168 cells, None)

HITM loads/tx at c 2/4/8, `all1`: MCS 92/110/111, FC 7.8/16.3/22.1, FC-PQ
10.9/15.3/9.4, U-SCL 2.3-2.5, and the Mutex/Condvar controls 1.0-1.3.
`half1_half64`: MCS 111/125/128, FC 12.5/23.6/30.3, FC-PQ 14.0/17.8/11.0.
LLC misses/tx ≤ 1.7 (`all1`) and ≤ 10 (`half1_half64`). Per-counter tables
with the clock table are in `analysis-perf/summary.md`.

## Comparison with the fixed-body non-LTO build

At the same placement, power setup (S1 3.0 GHz), kernel and rustc,
`new/old` median tx/s per cell (formal vs formal-03; perf vs perf-03):

- **Every variant is slower in absolute terms, upstream included.** `all1` None:
  upstream 0.937-0.944, patched variants 0.909-0.930 (formal). In the perf cohort
  every one of the 28 `all1` cells has ranges disjoint from perf-03 (upstream
  0.936-0.944, patched 0.906-0.928). `half1_half64` c1: upstream 1.01, patched
  0.95.
  User-mode instructions/tx **fell** (c1 `all1`: upstream 221.5k → 209.7k,
  patched ≈214.5k → ≈209.2k), and so did cycles/tx (upstream 84.6k → 79.9k,
  patched ≈82k → ≈79.8k). The lost wall time is therefore outside user mode or
  outside the counted span [INFERENCE]. Kernel, filesystem, NUMA policy,
  rustc and clock match, so the cause is **unattributed**. The possibilities
  are the harness/LTO change (upstream was rebuilt too) and host drift since
  2026-09-29 early morning. An interleaved A/B of `redb-build-04` against
  `redb-build-06` binaries would separate them; it was not run.

### Conclusions that changed

1. **Patched vs upstream when uncontended: "a few percent faster" → parity.**
   Old: upstream_gate/upstream 1.024 (`all1`) and 1.064 (`half1_half64`, outside
   noise) at 1 client (smoke-01), and in formal-03 the delegated variants led
   upstream by +4.4 % (`all1` c1: 33.5k vs 32.1k) and +5-7 % (`half1_half64` c1).
   Now upstream_gate/upstream is 0.998-1.006 in every None 1-client cell, and the
   delegated variants are within +0.5-1.1 % of upstream. The upstream harness now
   executes the same instructions/tx as patched (209.7k vs 209.2k; before,
   upstream ran 3.3 % more). This matches the README's reason for thin LTO.
2. **Absolute throughput at 3.0 GHz is 6-9 % lower** (above; unattributed).
   Absolute numbers from redb-internal must not be quoted for this build.
3. **FC-PQ vs U-SCL records/s, `half1_half64` None, 4 clients: 0.92 → 1.03.**
   FC-PQ no longer trails U-SCL there (8 clients unchanged at 1.27). FC-PQ's
   repeat range is 5 %, so this is a marginal change in sign, not a new
   effect. FC-PQ/FC records/s at 4 clients 0.67 → 0.75, FC-PQ/MCS 0.72 → 0.81:
   FC-PQ's records/s deficit against the FIFO locks is smaller.
4. **Immediate `half1_half64` 8 clients: FC-PQ's tx/s lead is gone.** Old
   FC-PQ/FC 1.11 and FC-PQ/MCS 1.21, now 0.96 and 1.00. FC-PQ's new repeat
   range is 31 % (7,378 median), so the control cell is noisy rather than
   reversed. Immediate `all1` 4 clients FC-PQ/MCS 0.99 → 1.07 (MCS range 10 %).
5. **Against the S0 smoke-01** (not the fixed-clock runs), the S0 conclusions
   "FC-PQ delivers 2.2-3.7× U-SCL" and the bimodal `all1` at 4-8 clients no
   longer appear. FC-PQ/U-SCL is 0.94-1.27 and every lock-cell range is ≤ 5 %.
   formal-03 had already attributed both to the S0 clock; the rerun confirms
   that under S1 with the new build.
6. **New baseline counter residue at 1 client:** HITM-supplied lines/tx 0.1 →
   ≈8 (`all1`) and 0.3 → ≈6.6 (`half1_half64`), and LLC misses/tx 0.2 → 0.6-0.8
   (`all1`), for every variant including upstream. The lock-dependent pattern
   (MCS ≫ FC > FC-PQ ≫ Mutex/Condvar, U-SCL) is unchanged. The new constant is
   unattributed (the clock sampler also ran in perf-03, so it is not the
   difference).

### Conclusions that held

- `service_jain` order in `half1_half64`: U-SCL 1.000 > FC-PQ 0.948-0.997 >
  MCS 0.864-0.869 ≈ FC 0.828-0.844 ≫ Mutex/Condvar controls (old: FC-PQ
  0.941/0.995/0.946, MCS 0.854-0.859, FC 0.833-0.850). Immediate has the same
  order. `half1_half8` is fair for all four locks (≥ 0.989).
- FC-PQ serves the 64-record half less (long share 0.51-0.61 vs FC/MCS
  0.69-0.73), so it completes more transactions and fewer records than FC.
  FC-PQ/FC tx/s `half1_half64` None 1.12/1.30/1.21 (old 1.11/1.24/1.21).
- FC-PQ/MCS `all1` None 1.15/1.17/1.23 (old 1.17/1.18/1.24). FC-PQ/FC `all1`
  0.99-1.09 (old 0.99-1.09). FC-PQ/U-SCL `all1` 1.00-1.05 (old 1.00-1.05).
- The fast path is not visible in throughput: the hit rate is the same (1.000 / ≈0.16-0.22 / 0.000),
  and all seven variants are within 1.1 % at 1 client.
- Only MCS migrates heavily (≈92-128 HITM loads/tx from 2 clients on); FC
  grows with clients, FC-PQ stays flat and low, and the controls and U-SCL are near
  zero. Values are within ≈10 % of perf-03.
- The Mutex/Condvar controls stay unfair (`service_jain` down to ≈1/c) and
  noisy in the half cohorts; beyond 1 client `upstream_gate_vs_upstream` carries no
  build-overhead information.

## Caveats

- Three repetitions; ranges are not confidence intervals. 10.7-23.8 % of cells
  are clock off-target (all counted in the tables; `*_not_flagged` fields in
  `summary.json` exclude them).
- formal-03 had no `upstream_gate`/`std_mutex`; their new/old comparison uses
  smoke-01 (S0) and perf-03 only.
- `build.json` git fields describe the default workspace, not this source (see
  Build and provenance).
