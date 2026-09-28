# Locks: service-fair delegation

A lock is two sequences: the clients that receive service and the cores that
execute it. Caller-executed locks tie them together: choosing who is served
next also chooses where the protected working set D lives next. That leaves a
caller-executed lock three options: switch executor every operation and pay
the migration M(W, topology) each time (MCS, ticket, CLH, service-fair CFL);
bias selection toward nearby cores to avoid migration, which is exactly the
freedom a service-fair policy does not have (CNA, ShflLock, CFL's NUMA
grouping); or refuse service to over-consumers and idle with backlog
(SCL/U-SCL slices and bans, non-work-conserving). Delegation removes the
executor from the selection: a combiner runs every critical section, so
changing who is charged costs a request handoff independent of D, and a
usage-ordered selection (FC-PQ) can be service-fair per request while staying
work-conserving. The advantage is amortized, not free: with batch occupancy b
and per-request delegation cost d, delegation wins only when
b(M + h - d) > M + A + K (handoff cost h, pass administration A, scheduler
state K); if d >= h + M no batch wins. Predicted advantage grows with D/CS and
with b; it vanishes at small D or low contention.

Narrative is not yet fixed: this paragraph is candidate B; candidate A ("fair service at a price") and the relationship between them are in [`docs/story-candidates.md`](docs/story-candidates.md).

Formal version: [`analysis/logp/`](analysis/logp/README.md) (`story-draft.tex`
for the narrative, `performance.tex` for the cost model and crossover,
`fairness.tex` for the service-spread results and the limits of current FC-PQ).

This repository holds the Rust delegation locks (`crates/libdlock`), the
microbenchmark CLI, C reference locks, and the UpScaleDB/redb integrations used
to test that thesis.

## Claims to falsify

- **H1** Caller-executed locks (MCS, ticket, CLH, CFL) lose throughput as the
  protected working set D grows, at fixed CS length and fixed fairness;
  locality-biased variants recover throughput only by giving up service share.
- **H2** U-SCL keeps fairness by idling with backlog; this is measurable as
  idle-with-backlog time.
- **H3** FC/FC-PQ throughput is flat in D (combiner-local data) while service
  Jain stays >= 0.95 under 1:8 request cost heterogeneity.
- **H4** The crossover D\* where delegation overtakes the best switching lock
  shrinks as fairness granularity tightens.
- **H5** In a real DB, the same ordering holds in UpScaleDB and redb with
  threads <= CPUs (waiters spin only).

## What existing evidence says

Index: [`docs/evidence/README.md`](docs/evidence/README.md). Raw data lives in
other branches.

| Verdict | Evidence |
| --- | --- |
| Supports (work-conservation) | U-SCL H2 reservation wait ~2.18 ms with a waiting requester vs FC-PQ ~0.6 us. |
| Supports (work-conservation) | U-SCL backlog ~460K at 1.1x load. |
| Supports | Hotspot, 32 workers: FC-PQ/MCS 0.743 uniform -> 1.135 hot90. |
| Supports | P8 fixed-work: FC 1.73x native. |
| Supports (E0(b)) | In the E0(b) ablation ([PR #48](https://github.com/taooceros/Locks/pull/48), open; default-off feature), `fcpq_fast_path` removes the 1-worker FC-PQ tax: 64.7 ns -> -8.1 ns per request vs FC; 1W FC-PQ/FC 0.53 -> 1.12. |
| Does not yet support | Saturated FC-PQ still costs more per op than FC: E0(b) measures 32W FC-PQ/FC 0.65 (cs 1) / 0.88 (cs 1000), and the fast path leaves saturated per-op cost within 2-5 ns ([PR #48](https://github.com/taooceros/Locks/pull/48)); P8 FC-PQ 13% below FC at identical CPU-s ([finding 003](docs/findings/003-eight-core-eight-worker-upscaledb.md)). |
| Does not yet support | No cache-migration counters anywhere; D_migrate is inferred, not measured. |
| Does not yet support | CFL-local (`cfl_local`, the Rust port) is an unverified proxy for CFL and matched/beat FC at P8. |

DLock2 waiters spin only, so every experiment keeps threads <= CPUs; with more
threads than CPUs the spinners starve a preempted combiner and throughput
collapses ([finding 002](docs/findings/002-dlock2-spin-wait-oversubscription-collapse.md)).

## Plan

Ordered experiments. E0 is engineering, not results. E1 is the core figure.

### E0 Prerequisites

- (a) Shelved, no longer a prerequisite: spin-then-park waiters in
  `crates/libdlock/src/dlock2/fc/lock.rs` and
  `crates/libdlock/src/dlock2/fc_pq/lock.rs`. Parking is deferred; waiters
  stay spin-only and every experiment keeps threads <= CPUs. Unmerged:
  spin-then-park change `uwqronmm` (`spin_park` feature) and the E0(a')
  timed-park comparison `zlqxmnyz`.
- (b) Trim the FC-PQ per-request tax: sample `__rdtscp` every k requests,
  bound heap arity. Target: FC-PQ/FC >= 0.95 at 1 worker. Status: met by
  `fcpq_fast_path` in [PR #48](https://github.com/taooceros/Locks/pull/48)
  (1W FC-PQ/FC 1.12); features are default-off and the default is still to
  be decided.
- (c) Obtain and validate the real CFL (Park/Eom, PPoPP'24) instead of
  `cfl_local`. Neither the `cfl` target (Rust port) nor `cflc` (local
  fairnumas adaptation in `c/cfl/`) is a verified paper implementation.

### E1 Working-set sweep (core figure)

- Workload: synthetic critical section touching W cache lines,
  W in {1, 4, 16, 64, 256, 1024, 4096}, fixed compute per CS.
- Threads: 8/16/32, one per physical core. Request cost heterogeneity 1:8 on
  half the threads.
- Locks: MCS, ticket, CLH, CFL, U-SCL, FC, FC-PQ.
- Metrics: throughput, service Jain, idle-with-backlog time, per-op L2/LLC
  misses and HITM via `perf stat`; for FC/FC-PQ also actual batch occupancy b,
  executor changes per pass p, and wasted pops k/b (the model's amortization
  inputs).
- Prediction: caller-executed locks fall with W, FC/FC-PQ flat once b is large
  enough, U-SCL Jain high but idle grows. Headline = crossover W\*, and the
  small-W regime where d >= h + M and no delegation variant wins.
- Placement: same-socket and cross-socket (NUMA) so D_migrate is non-trivial.
- Harness: `src/benchmark` counter-array style workload (`counter-array`, the
  existing `counter-proportional` with a data footprint; `--array-size`), as an
  `experiment.nu` group.

### E2 Fairness-granularity sweep

- Vary CFL/U-SCL slice length and the FC-PQ selection window.
- Plot throughput vs achieved maximum usage gap.
- Prediction: FC-PQ sits off the switching locks' trade-off curve.

### E3 Database confirmation (spin-only, threads <= CPUs)

- UpScaleDB single-operation integration with preload size (1K vs 1M records)
  as the W knob, plus perf counters.
- redb 1/64 write-transaction mix as the application endpoint, regenerated
  with the internal-lock harness ([`integration/redb/README.md`](integration/redb/README.md)).
- Both via the [`integration/README.md`](integration/README.md) workflows.

### E4 SCL-fidelity (only if E1-E3 hold)

- Disk-backed UpScaleDB with fsync, 4 find + 4 insert on 8 CPUs
  (threads <= CPUs), 120 s, lock-opportunity accounting. The original SCL
  setup (4 + 4 on 4 CPUs) oversubscribed.

## Success criteria

- E1: crossover W\* exists on both same-socket and cross-socket, with ranges
  not overlapping over >= 5 trials; FC-PQ Jain >= 0.95 at all W; U-SCL
  idle-with-backlog > 0 where switching locks idle 0.
- E3: the ordering from E1 reproduced in UpScaleDB and redb.
- Kill criterion: if real CFL is flat in W on cross-socket, the thesis reduces
  to work-conservation vs U-SCL only; record and reframe.

## Non-goals

- batch8 / application restructuring.
- More NUMA/SMT/hotspot cells.
- New third-party locks beyond those listed.
- Energy claims.

## Repository map

- `crates/libdlock/` - lock implementations; `dlock2/` is primary (`dlock/` is
  the legacy callback API, `parker/` thread parking strategies).
- `src/` - microbenchmark CLI (`src/benchmark/`, see `src/benchmark/README.md`).
- `c/` - C reference implementations, including `u-scl/`, `cfl/`, `shfllock/`.
- `integration/` - UpScaleDB + redb integrations; see
  [`integration/README.md`](integration/README.md).
- `experiment.nu`, `profile.nu` - sweep scripts.
- `visualization/*.py` - analysis and plotting.
- `docs/evidence/` - index of completed studies (raw data in other branches).
- `docs/findings/` - numbered findings.
- `docs/related-work/` - CFL, ShflLock, Syncord, TCLocks notes.
- `docs/archive/` - pre-thesis documents.
- `plan/` - dated plans; [`plan/index.md`](plan/index.md).

## Version control

`~/Locks` is a single colocated [jj](https://jj-vcs.github.io) repo (`.jj` and
`.git` side by side; bookmark `main` tracks `main@origin`). Historical
experiment branches exist only as bookmarks (`jj bookmark list`); their
git-ignored raw data was archived to `~/Locks-artifacts/<branch-name>/` (see
[`docs/evidence/README.md`](docs/evidence/README.md)). Materialize a branch with
`jj new <bookmark>` or, for a parallel checkout, `jj workspace add ../<name>
-r <bookmark>`; do not use `git worktree`. Build with `devenv shell -- cargo
...`; plain `cargo` lacks the `clang` linker.

## Build and run

### Requirements

- **Nightly Rust** (uses `sync_unsafe_cell`, `type_alias_impl_trait`, `thread_id_value`, etc.)
- **GCC or Clang** (compiles C reference implementations via `build.rs`)
- **x86_64 only** (uses `__rdtscp` for cycle-accurate timing)

### Build

```bash
cargo build --release
cargo test --release --verbose
cargo fmt --check
cargo clippy --release
```

### CLI

```
dlock [GLOBAL_OPTIONS] <EXPERIMENT>

EXPERIMENTS:
    d-lock2   <DLock2Experiment>      # Function-delegate API (primary)
    d-lock1   <DLock1Experiment>      # Callback-based API (legacy)

GLOBAL OPTIONS:
    -t, --threads <N,...>      Thread counts to test [default: CPU count]
    -c, --cpus <N,...>         CPU counts to test [default: CPU count]
    -o, --output-path <path>   [default: .worktree/output]
    -d, --duration <secs>      Measurement duration [default: 5]
    --warmup <secs>            Warmup period (no stats) [default: 2]
    --trials <N>               Independent trials [default: 1]
    --stat-response-time       Collect per-op latency CDFs
    -v, --verbose
```

### DLock2 experiments

- `counter-proportional` - shared counter with configurable CS length (`--cs`, `--non-cs`)
- `counter-array` - counter over a protected array; each CS iteration touches a distinct `u64` (`--array-size`, `--random-access`)
- `fetch-and-multiply` - multiply on shared counter
- `queue` - enqueue/dequeue on shared queue
- `priority-queue` - insert/extract-min on shared PQ
- `hash-map` - get/put/scan mix with scanner threads

### DLock2 lock targets (`--lock-targets`)

`fc`, `fc-ban`, `cc`, `cc-ban`, `dsm`, `fc-sl`, `fc-pq-b-tree`, `fc-pq-b-heap`,
`mutex`, `spin-lock`, `uscl`, `fc-c`, `cc-c`, `mcs`, `shfl-lock`, `shfl-lock-c`,
`cfl`, `cflc`, `ticket`, `clh`, `pthread-mutex`

### Examples

```bash
cargo run --release -- d-lock2 counter-proportional --cs 1000,3000 --non-cs 0
cargo run --release -- d-lock2 counter-proportional --lock-targets fc,fc-pq-b-tree --cs 1000 -t 4,8,16
cargo run --release -- d-lock2 counter-array --lock-targets mcs,cfl,uscl,fc,fc-pq-b-heap --cs 100 --array-size 4096 -t 8,16,32
cargo run --release -- --help
```

### Output

Arrow IPC `.arrow` files are written to `<output_path>/<lock_name>/`. New runs
default to `.worktree/output/`, an ignored per-worktree artifact area; analysis
scripts still read `visualization/output/` from older checkouts. Each file
contains per-thread records: loop counts, latencies, hold times, JFI, combiner
stats.

### Justfile shortcuts

```bash
just build                    # cargo build --release
just run2                     # d-lock2 counter-proportional --cs 1000,3000 --non-cs 0
just run2 "fc,fcpq"           # specific lock targets
just run1                     # d-lock1 variant
```
