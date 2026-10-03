# redb: client scalability beyond 8 and reader/writer interaction

Status: done 2026-09-30 (see Outcome), workspace `redb-scale-rw`
(`.worktree/jj/redb-scale-rw`), branch `experiment/redb-scale-rw`.
Evidence: `docs/evidence/redb-scale-rw-2026-09-29/`.

## Questions

**(A) Scalability.** Does any lock collapse, and does the service-fair ordering
survive, beyond 8 clients, including across the socket boundary (64 clients)?

**(B) Readers.** Readers (`begin_read`, point `get`s, a short scan) stay outside
every write lock, as in upstream. Does reader throughput or latency depend on the
writer lock, and do FC/FC-PQ combiners (writer bodies run on another thread, the
combiner spinning hot) interfere with readers differently from requester-run locks
(MCS, U-SCL, Mutex)? Does adding readers change writer tx/s or service Jain?

## Design

- **Harness (`integration/redb/src/main.rs`).** `MAX_WORKERS` 8 -> 64. New optional
  `--reader-cpus` (distinct from `--cpus`; the process affinity must be exactly
  writer CPUs plus reader CPUs). Each reader pinned, runs until the writers' window
  ends. One read transaction = `begin_read` + open table + 1 lookup of the last key
  of a random writer already known to have data (every 64th transaction probes a
  random writer to discover more; a first version that picked a random writer every
  time made reader ops/s depend on writer fairness, because unfair locks leave most
  writers without keys, and was discarded) + `G=4` point gets of random keys that exist in
  that snapshot (keys of a writer are contiguous 0..max, so every get must hit and
  carry the exact payload) + one range scan of `S=16` entries (contiguous, exact
  payloads). Any miss or wrong payload fails the cell. No counter is shared with
  the writers (a shared "committed" counter would add coherence traffic to the
  writers and confound the R sweep); keys come from the reader's own snapshot.
- **Reader metrics.** read txn/s, ops/s (gets + scans), begin_read and whole-txn
  latency in a log2 x 8-sub-bucket histogram (p50/p99 upper bounds, <=12.5 % wide),
  snapshot staleness sampled every 16th read txn: records committed between the
  snapshot and a second `begin_read` taken when the first read txn's reads are done
  (`table.len()` difference; redb exposes no public read-transaction id). Equals
  committed transactions for `all1`.
- **Runner (`integration/redb/scale_rw.py`, new).** Imports the power, clock,
  sampler, perf and metric helpers from `run.py` (additive only there). Per-cell
  CPU sets: writers = first W of CPUs 0..63 (physical cores; node 0 = 0-31, node 1 =
  32-63), readers = CPUs 8.. (same socket, physical cores, distinct from writers).
  `numactl --membind=0` for every cell (memory on node 0 at all W, so the only
  thing that changes at W>32 is that half the clients are remote; stated, not
  interleaved). Cells with CPUs on both nodes are marked `cross_socket`. Runner
  itself is pinned to CPU 127 (idle SMT sibling) outside client CPUs. Measurement
  lock taken exclusively per (cohort, W, R) group of 7 variants, not for the whole
  campaign, so the sibling's runs can interleave between groups.
- **Matrix.** A: W in {1,2,4,8,16,32,64}, cohorts `all1` and `half1_half64`,
  None, 7 variants, 3 repeats (294 cells). B: `all1`, None; forward W=8 with R in
  {0,1,4,8,16}; reverse R=8 with W in {1,2,4,8}; 7 variants, 3 repeats (168
  cells incl. the shared W=8,R=8 cell once). Perf: mcs/fc/fc_pq at W=32 and W=64,
  `all1` and `half1_half64`; all 7 variants at W=8 with R=16 and the R=0 control
  (perf counters are process totals, so reader work is included in R=16 cells).
- **Gate.** `correctness.py` gains `trial-*` cases that run the timed trial binary
  itself under `taskset` for every variant: 64 writers across both sockets with
  exact contents and reopen (None and Immediate), writers with 16 readers (timed
  2 s, readers must see the table grow, zero misses), Immediate with readers.
- **Trims (pre-agreed).** If machine time exceeds ~3 h, repeats drop to 2 for the
  64-client and cross-socket cells first; report it.

## Acceptance

Gate green (known intermittent transfer-Immediate integrity failure excepted and
named); matrix and perf cohorts with 0 failed cells; RESULTS.md and report.typ
answering: throughput and Jain vs clients to 64 with the socket boundary marked,
any collapse, reader ops/s and p99 vs writer lock, FC/FC-PQ vs requester-run locks
for readers.

## Outcome

294 scale + 168 reader + 78 perf cells, 3 repeats each (no trims; about 25 min of
elapsed time), 0 failed; gate 146 cases (new `trial-*`), 146/146 on the second run, the
first run had one known intermittent (`std_mutex` `transfer-immediate`). Results:
[`docs/evidence/redb-scale-rw-2026-09-29/RESULTS.md`](../../docs/evidence/redb-scale-rw-2026-09-29/RESULTS.md).
Deviations: the first reader design (random writer per transaction) made reader ops/s
depend on writer fairness and was replaced (540 cells discarded); W >= 32 cells ran at
2.8 GHz because S1 cannot hold 3.0 GHz on 32+ busy cores (flagged, no root to change it).
