# redb: scalability to 64 clients and concurrent readers (2026-09-29/30)

Workspace `redb-scale-rw`, branch `experiment/redb-scale-rw`, plan
[`plan/2026-09-29/redb-scale-and-readers.md`](../../../plan/2026-09-29/redb-scale-and-readers.md).
Harness guide: `integration/redb/README.md` ("Scalability beyond 8 clients and concurrent
readers"). Report: `report.typ` (HTML-first; `typst compile --features html --format html
report.typ report.html`), figures from `make_figures.py`, flat medians in `medians.json`,
full per-cell tables in `tables-{scale,rw,perf}.md`.

Raw data stays under `.worktree/` in the `redb-scale-rw` workspace (git-ignored, not copied):

| Stage | Root | Cells | Failed |
|---|---|---:|---:|
| Build (upstream, patched, test_hooks) | `.worktree/redb-build-02/build.json` | — | — |
| Correctness gate | `.worktree/redb-build-02/correctness-01` (first run), `correctness-02` (second run) | 146 cases each | 1 (known intermittent, below); 0 |
| Scale (1-64 writers) | `.worktree/redb-scale-rw-02/scale`, `analysis-scale/summary.{json,md}` | 294 | 0 |
| Readers (W x R) | `.worktree/redb-scale-rw-02/rw`, `analysis-rw/summary.{json,md}` | 168 | 0 |
| Perf cohort | `.worktree/redb-scale-rw-02/perf`, `analysis-perf/summary.{json,md}` | 78 | 0 (every event 100 % running) |
| Sustain probe (64 cores spinning) | `.worktree/redb-scale-rw-01-sustain/sustain-20260930T170128/sustain.json` | — | — |
| Discarded first reader design | `.worktree/redb-build-01`, `.worktree/redb-scale-rw-01` (540 cells, 0 failed) | 540 | not used, see Deviations |

Build: patched-tree SHA-256 `9e57be76fba0ad56bbb8e9f0c0679ef46831de84ef1381204200b3e368306366`
(same crate `redb-3.1.0` plus the unchanged two patches), binaries `redb-upstream`
`ec735bc4…`, `redb-patched` `479b87fc…`, `redb-test_hooks` `6e4ed827…` (full hashes in
`build.json` and the root's `manifest.json`), rustc `1.100.0-nightly (6bb1652a0 2026-09-22)`,
thin LTO as in the closure rerun. `build.json` `git_head` is not this source (jj workspaces are
not colocated; same caveat as the closure rerun); the campaign manifest records jj's own
`jj_parent_commit` (`610dc3be…`, `evidence/redb-closure-rerun`) and working-copy commit.
Elapsed time of the final campaign: scale 811 s, readers 441 s, perf 223 s (about 25 min,
measurement-lock waits included), far below the 3 h budget: **no repeat was
trimmed, every cell has 3 repeats**.

## Method

- Host: 2 x Xeon Gold 6438M (32 cores x 2 SMT per socket), power setup **S1** (performance
  governor, min = max = 3.0 GHz, turbo on). Writers: one per **physical core**, CPUs 0..W-1
  (node 0 = CPUs 0-31; W = 64 adds CPUs 32-63 on node 1). **Cells with W > 32 are cross-socket**
  (`cross_socket` in every row). Readers: physical cores of node 0, CPUs 8..8+R-1 for W <= 8.
  The process affinity is exactly writers + readers.
- **Memory: `numactl --membind=0` in every cell** (not interleaved): at W = 64 the 32 clients on
  node 1 use node-0 memory. The runner is pinned to CPU 127 (SMT sibling of CPU 63, outside the
  client CPUs except as sibling of client 63 at W = 64). The measurement lock is taken
  exclusively per (cohort, W, R) group.
- Cohorts `all1` and `half1_half64`, durability None, all 7 variants, 3 repeats, variant order
  randomised per group. Each cell is a fresh process: exact-content verification, close/reopen
  and (for every durability, not only Immediate) an exact reopen are required, else the cell fails.
- Reader: one read transaction = `begin_read`, last key of a random writer that has data, 4 point
  gets of random keys of that writer, one scan of 16 consecutive keys; every get must hit with the
  exact payload, scans must be consecutive and exact (a miss fails the cell). Saturated (no think
  time). Reader ops = gets + scans (a scan is one op; counting every scanned entry instead gives 1.97M / 3.93M / 5.41M / 6.67M per second at R = 1 / 4 / 8 / 16, W = 8, median over all variants, `medians.json` key `reader_entry_ops_s`); empty read transactions are 0.025 % of all; latency percentiles are fine-histogram bucket upper bounds
  (<= 12.5 % wide); staleness = records committed between a snapshot and a second `begin_read`
  after the transaction's reads (every 16th transaction; redb has no public read-transaction id).
- Perf cohort: `perf stat` process totals, user mode, counting only the client phase (as in the
  closure rerun). In R > 0 cells the readers are part of the totals.

## Clock

The 3.0 GHz fixed clock **holds up to 16 busy cores but not beyond**: with W = 32 or 64 busy cores
every cell runs at 2.79-2.80 GHz (sampler and perf `effective_ghz` agree; the sustain probe with
all 64 cores spinning gave 2.797-2.800 GHz per CPU, all below the 3.0 +- 2 % tolerance). I have no
root and cannot lower F, so the cells are kept and flagged: `clock_off_target` is true for 36/36
decided cells at W = 32 and 36/41 at W = 64 (the rest are U-SCL cells whose clients sleep and
report an idle clock); 0-2 cells at W <= 16. The uniform 2.8 GHz affects all variants alike;
`tx_s_at_ref` (tx/s x 3.0 / clock) is in `tables-scale.md` and summary.json and assumes the
serial path scales with the clock. Conclusions below compare variants at the same W, and the
W = 16 -> 32 step mixes a 6.7 % clock drop.

## Headline tables (medians of 3; ranges in `tables-*.md`)

**tx/s (k), all1, None, W writers**

| variant | 16 | 32 (one socket full) | 64 (two sockets) |
|---|---:|---:|---:|
| `upstream` | 26.8 | 25.3 | 24.8 |
| `upstream_gate` | 27.0 | 25.4 | 25.0 |
| `std_mutex` | 27.2 | 25.4 | 25.4 |
| `mcs` | 20.5 | 18.3 | 10.6 |
| `uscl` | 25.1 | 24.5 | 23.3 |
| `fc` | 24.1 | 23.4 | 23.9 |
| `fc_pq` | 25.8 | 24.2 | 24.3 |

**Service-time Jain, all1**

| variant | 16 | 32 (one socket full) | 64 (two sockets) |
|---|---:|---:|---:|
| `upstream` | 0.093 | 0.031 | 0.044 |
| `upstream_gate` | 0.084 | 0.057 | 0.030 |
| `std_mutex` | 0.156 | 0.081 | 0.031 |
| `mcs` | 0.999 | 0.999 | 0.982 |
| `uscl` | 1.000 | 1.000 | 1.000 |
| `fc` | 0.995 | 0.988 | 0.983 |
| `fc_pq` | 1.000 | 1.000 | 1.000 |

**Tx-count Jain, all1**

| variant | 16 | 32 (one socket full) | 64 (two sockets) |
|---|---:|---:|---:|
| `upstream` | 0.091 | 0.031 | 0.045 |
| `upstream_gate` | 0.082 | 0.057 | 0.030 |
| `std_mutex` | 0.156 | 0.083 | 0.031 |
| `mcs` | 1.000 | 1.000 | 1.000 |
| `uscl` | 1.000 | 1.000 | 0.999 |
| `fc` | 1.000 | 1.000 | 1.000 |
| `fc_pq` | 0.998 | 1.000 | 0.999 |

**records/s (k), half1_half64**

| variant | 16 | 32 (one socket full) | 64 (two sockets) |
|---|---:|---:|---:|
| `upstream` | 623 | 590 | 24 |
| `upstream_gate` | 27 | 337 | 581 |
| `std_mutex` | 427 | 590 | 464 |
| `mcs` | 376 | 330 | 259 |
| `uscl` | 296 | 284 | 276 |
| `fc` | 418 | 397 | 401 |
| `fc_pq` | 321 | 298 | 298 |

**Service-time Jain, half1_half64**

| variant | 16 | 32 (one socket full) | 64 (two sockets) |
|---|---:|---:|---:|
| `upstream` | 0.062 | 0.031 | 0.016 |
| `upstream_gate` | 0.062 | 0.075 | 0.020 |
| `std_mutex` | 0.114 | 0.042 | 0.024 |
| `mcs` | 0.879 | 0.885 | 0.901 |
| `uscl` | 1.000 | 1.000 | 1.000 |
| `fc` | 0.865 | 0.858 | 0.842 |
| `fc_pq` | 0.999 | 1.000 | 1.000 |

**Service share of the 64-record clients, half1_half64 (fair = 0.5)**

| variant | 16 | 32 (one socket full) | 64 (two sockets) |
|---|---:|---:|---:|
| `upstream` | 1.000 | 1.000 | 0.000 |
| `upstream_gate` | 0.000 | 0.544 | 1.000 |
| `std_mutex` | 0.663 | 1.000 | 0.767 |
| `mcs` | 0.685 | 0.680 | 0.663 |
| `uscl` | 0.502 | 0.502 | 0.501 |
| `fc` | 0.696 | 0.701 | 0.712 |
| `fc_pq` | 0.507 | 0.504 | 0.503 |

**Mean writer-CPU clock (GHz), all1 (target 3.0; — = clients mostly asleep, undecidable)**

| variant | 16 | 32 (one socket full) | 64 (two sockets) |
|---|---:|---:|---:|
| `upstream` | 2.98 | 2.80 | 2.80 |
| `upstream_gate` | 2.99 | 2.80 | 2.80 |
| `std_mutex` | 2.98 | 2.80 | 2.80 |
| `mcs` | 2.99 | 2.79 | 2.79 |
| `uscl` | — | — | 2.99 |
| `fc` | 2.98 | 2.79 | 2.80 |
| `fc_pq` | 2.99 | 2.80 | 2.80 |

**Writer tx/s (k), W=8, all1, vs R readers**

| variant | R=0 | R=1 | R=4 | R=8 | R=16 |
|---|---:|---:|---:|---:|---:|
| `upstream` | 26.1 | 19.9 | 13.8 | 9.8 | 5.5 |
| `upstream_gate` | 26.7 | 20.0 | 14.2 | 10.3 | 5.0 |
| `std_mutex` | 26.9 | 20.3 | 14.0 | 10.2 | 5.0 |
| `mcs` | 22.1 | 17.7 | 13.0 | 9.2 | 4.5 |
| `uscl` | 25.6 | 19.0 | 13.4 | 9.8 | 4.7 |
| `fc` | 24.6 | 18.7 | 12.7 | 9.6 | 4.9 |
| `fc_pq` | 27.3 | 20.3 | 13.7 | 10.0 | 5.0 |

**Reader ops/s (k) (gets + scans), W=8**

| variant | R=1 | R=4 | R=8 | R=16 |
|---|---:|---:|---:|---:|
| `upstream` | 486 | 977 | 1391 | 1756 |
| `upstream_gate` | 502 | 977 | 1357 | 1707 |
| `std_mutex` | 486 | 977 | 1346 | 1745 |
| `mcs` | 504 | 1015 | 1421 | 1709 |
| `uscl` | 513 | 1024 | 1401 | 1705 |
| `fc` | 497 | 994 | 1392 | 1732 |
| `fc_pq` | 492 | 995 | 1369 | 1749 |

**Reader read-transaction p50 (µs, bucket upper bound), W=8**

| variant | R=1 | R=4 | R=8 | R=16 |
|---|---:|---:|---:|---:|
| `upstream` | 7.2 | 15.4 | 22.5 | 36.9 |
| `upstream_gate` | 7.2 | 15.4 | 22.5 | 36.9 |
| `std_mutex` | 7.2 | 15.4 | 22.5 | 36.9 |
| `mcs` | 6.7 | 14.3 | 20.5 | 36.9 |
| `uscl` | 6.7 | 13.3 | 20.5 | 36.9 |
| `fc` | 7.2 | 14.3 | 20.5 | 36.9 |
| `fc_pq` | 7.2 | 14.3 | 20.5 | 36.9 |

**Reader read-transaction p99 (µs, bucket upper bound), W=8**

| variant | R=1 | R=4 | R=8 | R=16 |
|---|---:|---:|---:|---:|
| `upstream` | 26.6 | 53.2 | 81.9 | 114.7 |
| `upstream_gate` | 24.6 | 53.2 | 81.9 | 114.7 |
| `std_mutex` | 26.6 | 53.2 | 81.9 | 114.7 |
| `mcs` | 28.7 | 53.2 | 81.9 | 122.9 |
| `uscl` | 28.7 | 53.2 | 81.9 | 122.9 |
| `fc` | 28.7 | 53.2 | 81.9 | 114.7 |
| `fc_pq` | 26.6 | 53.2 | 81.9 | 114.7 |

**Mean snapshot staleness (records behind), W=8**

| variant | R=1 | R=4 | R=8 | R=16 |
|---|---:|---:|---:|---:|
| `upstream` | 0.21 | 0.28 | 0.28 | 0.25 |
| `upstream_gate` | 0.20 | 0.29 | 0.31 | 0.22 |
| `std_mutex` | 0.21 | 0.29 | 0.30 | 0.24 |
| `mcs` | 0.17 | 0.26 | 0.25 | 0.21 |
| `uscl` | 0.19 | 0.26 | 0.28 | 0.22 |
| `fc` | 0.19 | 0.25 | 0.28 | 0.22 |
| `fc_pq` | 0.20 | 0.27 | 0.29 | 0.22 |

**Reverse view, R=8: reader ops/s (k)**

| variant | W=1 | W=2 | W=4 | W=8 |
|---|---:|---:|---:|---:|
| `upstream` | 1337 | 1329 | 1366 | 1391 |
| `upstream_gate` | 1354 | 1364 | 1383 | 1357 |
| `std_mutex` | 1323 | 1367 | 1342 | 1346 |
| `mcs` | 1333 | 1394 | 1369 | 1421 |
| `uscl` | 1333 | 1339 | 1356 | 1401 |
| `fc` | 1341 | 1363 | 1378 | 1392 |
| `fc_pq` | 1352 | 1355 | 1364 | 1369 |

**Reverse view, R=8: reader p99 (µs)**

| variant | W=1 | W=2 | W=4 | W=8 |
|---|---:|---:|---:|---:|
| `upstream` | 81.9 | 81.9 | 81.9 | 81.9 |
| `upstream_gate` | 81.9 | 81.9 | 81.9 | 81.9 |
| `std_mutex` | 81.9 | 81.9 | 81.9 | 81.9 |
| `mcs` | 81.9 | 73.7 | 81.9 | 81.9 |
| `uscl` | 81.9 | 81.9 | 81.9 | 81.9 |
| `fc` | 81.9 | 81.9 | 81.9 | 81.9 |
| `fc_pq` | 81.9 | 81.9 | 81.9 | 81.9 |

**Reverse view, R=8: writer tx/s (k)**

| variant | W=1 | W=2 | W=4 | W=8 |
|---|---:|---:|---:|---:|
| `upstream` | 9.4 | 9.4 | 9.5 | 9.8 |
| `upstream_gate` | 9.2 | 9.0 | 9.8 | 10.3 |
| `std_mutex` | 9.1 | 9.3 | 9.2 | 10.2 |
| `mcs` | 9.3 | 8.7 | 8.7 | 9.2 |
| `uscl` | 9.2 | 8.9 | 9.2 | 9.8 |
| `fc` | 9.4 | 9.0 | 9.2 | 9.6 |
| `fc_pq` | 9.6 | 9.0 | 9.2 | 10.0 |

**Reverse view, R=8: writer service Jain**

| variant | W=1 | W=2 | W=4 | W=8 |
|---|---:|---:|---:|---:|
| `upstream` | 1.000 | 0.505 | 0.351 | 0.197 |
| `upstream_gate` | 1.000 | 0.963 | 0.559 | 0.270 |
| `std_mutex` | 1.000 | 0.586 | 0.264 | 0.178 |
| `mcs` | 1.000 | 1.000 | 1.000 | 1.000 |
| `uscl` | 1.000 | 1.000 | 1.000 | 1.000 |
| `fc` | 1.000 | 1.000 | 0.998 | 0.998 |
| `fc_pq` | 1.000 | 1.000 | 1.000 | 1.000 |

## Perf cohort (HITM, L2/LLC, IPC)

Medians of 3 (`tables-perf.md` has ranges and more counters). Process totals per committed writer
transaction; W = 32/64 have no readers, the W = 8, R = 16 rows include the readers' work.

| cohort | W | R | variant | HITM loads/tx | L2-miss loads/tx | LLC miss/tx | IPC |
|---|---:|---:|---|---:|---:|---:|---:|
| all1 | 8 | 0 | `upstream` | 1 | 1 | 0.9 | 2.59 |
| all1 | 8 | 0 | `upstream_gate` | 1 | 3 | 1.1 | 2.57 |
| all1 | 8 | 0 | `std_mutex` | 1 | 1 | 0.9 | 2.46 |
| all1 | 8 | 0 | `mcs` | 113 | 137 | 2.0 | 0.27 |
| all1 | 8 | 0 | `uscl` | 3 | 7 | 1.2 | 2.07 |
| all1 | 8 | 0 | `fc` | 23 | 26 | 1.3 | 0.25 |
| all1 | 8 | 0 | `fc_pq` | 9 | 11 | 1.0 | 0.27 |
| all1 | 8 | 16 | `upstream` | 2303 | 4308 | 2.3 | 0.51 |
| all1 | 8 | 16 | `upstream_gate` | 2779 | 5149 | 1.5 | 0.50 |
| all1 | 8 | 16 | `std_mutex` | 2788 | 5450 | 1.5 | 0.50 |
| all1 | 8 | 16 | `mcs` | 3574 | 6833 | 2.8 | 0.28 |
| all1 | 8 | 16 | `uscl` | 2901 | 5589 | 2.4 | 0.50 |
| all1 | 8 | 16 | `fc` | 2847 | 5531 | 2.5 | 0.25 |
| all1 | 8 | 16 | `fc_pq` | 2916 | 5711 | 2.4 | 0.25 |
| all1 | 32 | 0 | `mcs` | 115 | 142 | 2.4 | 0.13 |
| all1 | 32 | 0 | `fc` | 24 | 30 | 2.3 | 0.08 |
| all1 | 32 | 0 | `fc_pq` | 16 | 19 | 2.4 | 0.09 |
| all1 | 64 | 0 | `mcs` | 74 | 147 | 335.8 | 0.11 |
| all1 | 64 | 0 | `fc` | 17 | 28 | 149.2 | 0.05 |
| all1 | 64 | 0 | `fc_pq` | 16 | 24 | 141.6 | 0.05 |
| half1_half64 | 32 | 0 | `mcs` | 135 | 176 | 4.3 | 0.14 |
| half1_half64 | 32 | 0 | `fc` | 32 | 42 | 10.0 | 0.09 |
| half1_half64 | 32 | 0 | `fc_pq` | 20 | 24 | 6.8 | 0.09 |
| half1_half64 | 64 | 0 | `mcs` | 80 | 171 | 371.2 | 0.11 |
| half1_half64 | 64 | 0 | `fc` | 21 | 40 | 274.8 | 0.05 |
| half1_half64 | 64 | 0 | `fc_pq` | 15 | 28 | 229.1 | 0.05 |

## Conclusions

1. **Throughput is flat in the number of clients; only MCS collapses.** One writer runs at a time,
   so 1 -> 64 clients costs every lock except MCS 8-15 % of the 1-client tx/s
   (`all1`: 27.6-28.0k at 1 client; 64 clients: `fc_pq` 24.3k, `fc` 23.9k, `uscl` 23.3k,
   `std_mutex` 25.4k, `upstream` 24.8k; all at 2.8 GHz for W >= 32, the 1-client cells at 3.0).
   `mcs` degrades steadily (23.3k at 2 clients, 20.5k at 16, 18.3k at 32) and **collapses across the
   socket boundary: 10.6k at 64 clients (repeats 7.4k-15.6k)**, 0.38x its 1-client rate. Perf at
   W = 64 shows the mechanism's footprint: LLC misses/tx 336 for MCS (vs 2.4 at W = 32) against
   142-149 for FC/FC-PQ, and the body runs on the requesting thread, so I expect remote-socket clients to
   run the body against node-0 memory [INFERENCE]; service Jain is 0.98 at 64 although every
   client completes the same number of transactions.
2. **Fairness ordering from 8 clients survives to 64, including across sockets.** `fc_pq` and `uscl`
   keep service Jain 1.000 at 16, 32 and 64 clients in `all1` and in `half1_half64`
   (long-client share 0.50); `mcs` and `fc` keep transaction counts equal but give the 64-record
   clients 66-71 % of the lock time (Jain 0.84-0.90 at 64); the controls (`upstream`,
   `upstream_gate`, `std_mutex`) monopolise: Jain 0.02-0.08 at 32 and 64 clients while their tx/s
   is unaffected, which makes `half1_half64` records/s of the controls erratic (24k to 631k across W,
   decided by whether short or long clients win the lock).
3. **`fc_pq` is the only variant with service Jain 1.000 and `all1` tx/s within 5 % of the
   fastest at 64 clients** (24.3k vs `std_mutex` 25.4k; `uscl`, equally fair, is 8 % below the
   fastest at 23.3k). In `half1_half64` records/s at 64 clients it is 1.08x `uscl` (298k vs 276k)
   and 1.15x `mcs` (259k) but 0.74x `fc` (401k), whose lead comes from giving the 64-record
   clients 71 % of the lock time.
4. **Readers do not distinguish the writer locks.** At W = 8, reader ops/s are within 3-6 % across
   all 7 variants at every R (repeat range 3-5 %): 0.49-0.51M (R = 1), 0.98-1.02M (R = 4),
   1.35-1.42M (R = 8), 1.71-1.76M (R = 16). Read-transaction p99 is the same histogram bucket for
   all variants (R = 16: 114.7-122.9 us, one 12.5 % bucket apart; p50 36.9 us). The reverse view
   (R = 8, W = 1..8) is equally flat (1.32-1.42M ops/s, p99 73.7-81.9 us).
5. **FC/FC-PQ combiners interfere with readers no differently from requester-run locks**: reader
   ops/s of `fc`+`fc_pq` over `mcs`+`uscl` is 0.97-1.02x at every R (inside the repeat noise);
   writer tx/s relative to R = 0 is the same (R = 16: 0.18-0.21x for every variant); HITM loads/tx
   at R = 16 are 2.3-3.6k for all variants (vs 1-113 at R = 0). This is a measurement on separate
   physical cores; readers on the SMT siblings of a spinning combiner were not tried.
6. **Saturated readers slow every writer lock alike, and more than the lock choice matters**: at
   W = 8 the writers keep 0.74-0.80x (R = 1), 0.50-0.59x (R = 4), 0.37-0.42x (R = 8) and
   0.18-0.21x (R = 16) of their R = 0 tx/s, and at R = 8 the writers sit at 8.7-10.3k tx/s for
   every W in 1..8 and every variant (W = 1: 9.1-9.6k vs 27.5-28.0k alone). The service-fair locks keep
   service Jain >= 0.997 with readers; the controls stay unfair. A single diagnostic
   `perf record` (mcs, `all1`, W = 2, R = 8, HITM-load samples, not a cohort) attributes 48 % of
   cross-core modified-line loads to redb's `PagedCachedFile::read` and 17 % to contended
   `std::sync::Mutex` locking: the interference happens in redb's shared page cache and tracker
   mutex [INFERENCE from one cell], not in the write-lock algorithm. Mean snapshot staleness is
   0.17-0.31 records (about 28 % of sampled snapshots are at least one record behind by the end
   of the read transaction); the largest sampled gap in any cell was 32 at R = 1, 5 at R = 4,
   4 at R = 8 and 10 at R = 16.
7. **Gate and correctness**: 146/146 cases in the second gate run. The first run had 1 failure,
   `std_mutex` `transfer-immediate` ("integrity check after reopen had to repair the database"),
   the known, not root-caused intermittent of the closure rerun (there seen on MCS, FC-PQ and
   U-SCL); that case passed 12/12 in an immediate re-run and the second full gate passed. The
   new `trial-*` cases (64 and 40 writers across both sockets with exact contents and reopen,
   readers during writes) passed for every variant in both runs.

## Deviations and trims

- **No repeats were trimmed** (final campaign about 25 min of machine time).
- **First reader design discarded.** The first build (`redb-build-01`, run `redb-scale-rw-01`,
  540 cells) picked a random writer for each read transaction. Unfair locks leave most writers
  without keys, so 7/8 of the reader's transactions were empty and reader ops/s depended on the
  writer lock (upstream/`std_mutex` 0.15-0.43M vs 0.50M-1.7M). Readers now choose among writers
  already known to have data (one discovery probe per 64 transactions), the whole binary was
  rebuilt, re-gated and every cell re-run. Only the second campaign is reported.
- Clock: W >= 32 cells run at 2.80 GHz, not 3.0 (flagged, above).
- Staleness is a record-count gap from `table.len()` (every 16th read transaction), not a
  transaction-ID gap (no public read-transaction ID in redb).
- Memory bound to node 0 in all cells, not interleaved.
- Perf cohort: `all1` and `half1_half64` at W = 32/64 (mcs, fc, fc_pq); `all1` at W = 8 with R = 0
  and R = 16 for all 7 variants. The R = 16 counters include the readers; the R = 0 rows are the
  matched control.
- The runner sat on CPU 127 (sibling of client 63 at W = 64) rather than a fully idle core.
- `run.py` only changed `MAX_CLIENTS` 8 -> 64 (its sweep stays 1-8); the new matrix lives in
  `integration/redb/scale_rw.py`. `libdlock`, the variants, the bridge and the patches are unchanged.

## Not measured

Durability Immediate at scale, the transfer cohort with readers, readers on SMT siblings of
combiners, paced (non-saturated) readers, interleaved memory, a 3.0 GHz fixed clock with 32-64 busy
cores (the host cannot hold it), and any production trace.

## Reproduce

```sh
export MEASUREMENT_LOCK=$HOME/.cache/locks-experiments/measurement.lock
flock --shared "$MEASUREMENT_LOCK" python3 -m integration.redb.build --output-dir .worktree/redb-build-02 --jobs 16
flock --exclusive "$MEASUREMENT_LOCK" prlimit --core=0 python3 -m integration.redb.correctness \
  --build-dir .worktree/redb-build-02 --output-dir .worktree/redb-build-02/correctness-01
python3 -m integration.redb.scale_rw --prepare-only --build-dir .worktree/redb-build-02 \
  --output-root .worktree/redb-scale-rw-02 --power-setup S1 --fixed-ghz 3.0
for k in scale rw perf; do python3 -m integration.redb.scale_rw --run $k --output-root .worktree/redb-scale-rw-02; done
for k in scale rw perf; do python3 -m integration.redb.scale_rw --analyze $k --output-root .worktree/redb-scale-rw-02; done
python3 docs/evidence/redb-scale-rw-2026-09-29/make_figures.py .worktree/redb-scale-rw-02
```
