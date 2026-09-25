# Database experiment setup

The UpScaleDB integration changes **synchronization, not database operation
logic**. The redb experiment uses unmodified redb. Both are research harnesses,
not general-purpose database mutex replacements or production workload claims.

## Integration boundary

For UpScaleDB, the supported request remains **one original find or insert per
submission**. The adapter extracts the complete existing environment-mutex
critical-section body into a shared helper. The public API/refactored control
and delegated path call that same body. The Native baseline uses the original
upstream implementation. FC/FC-PQ may execute a request on a combiner; other lock
backends execute it on the requesting worker. Those execution/ordering differences
are the synchronization treatment being evaluated.

This is not an unmodified UpScaleDB binary or a transparent replacement of its
entire API. Integration source changes, caller-owned output buffers and the
restricted synchronous ABI are explicit. Supported operations use the same
in-memory, nontransactional, fixed-width key/record configuration across backends.
Unsupported transaction modes, flags or schemas are not silently approximated.
Callers must join before exclusive destruction; exceptions, recursive submissions,
longjmp, cancellation and unwinding may not cross the Rust/C ABI.

Not included: the batch8 extension that makes eight inserts one scheduling unit,
heterogeneous batch runners, split-environment/multitable application restructuring,
or report generators that require those omitted experiments. Historical research
worktrees and results remain intact. The earlier removal of all UpScaleDB
integration was an assistant interpretation error, not the intended scope.

## Environment and resource discipline

Run from the repository root on Linux/x86_64. Install Nix and devenv and initialize
the existing C submodule:

```sh
git submodule update --init --recursive
devenv shell
```

The environment provides nightly Rust, Clang/libclang, mold, Python, pyarrow,
matplotlib, numactl, Autoconf/Automake/libtool, GNU make, Git and util-linux
(`taskset`, `flock`, `findmnt`, `prlimit`). The UpScaleDB builder obtains pinned
Boost/GCC through Nix and upstream source through Git. Initial setup requires
network access; `--source-repository` can select a local upstream clone while
retaining commit/patch verification.

Inspect `lscpu -e=CPU,CORE,SOCKET,NODE` and the process affinity before choosing CPUs.
Historical CPU IDs are not universal defaults. Use distinct physical cores for
physical-core experiments and select SMT/oversubscription deliberately. Memory
binding may be prohibited in containers; qualify any unbound run.

All cooperating worktrees must use the same external measurement lock. Builds and
analysis acquire it shared; correctness/smoke and timed runs acquire it exclusively.
Disjoint CPU sets do not prevent shared cache, memory or power interference; this
convention cannot isolate unrelated host tasks.

```sh
mkdir -p "$HOME/.cache/locks-experiments"
export MEASUREMENT_LOCK="$HOME/.cache/locks-experiments/measurement.lock"
# Builds/analysis: flock --shared "$MEASUREMENT_LOCK" COMMAND ...
# Correctness/timed runs: flock --exclusive "$MEASUREMENT_LOCK" COMMAND ...
```

Use a new output root for each cohort and preserve failed attempts. Builds, native
sources, databases, binaries, raw results and analysis belong under ignored
`.worktree/` paths, not in Git.

## Source-level checks

These checks are not application performance results:

```sh
cargo build --workspace --release --locked
cargo test -p libdlock --release --lib dlock2_unit_test::ownership
cargo test -p upscaledb-bridge --release -- --test-threads=1
cargo test -p upscaledb-bridge --release --features profile,test-hooks -- --test-threads=1
python3 -m unittest discover -s integration/upscaledb -p 'test_*.py' -v
```

The bridge checks cover callback exclusion, synchronous results, error propagation,
reentry rejection, multiple handles and joined destruction. Non-Copy ownership
checks and real database correctness/smoke remain separate requirements.

## UpScaleDB: single-operation integration

The builder pins upstream commit `cb124e1f91601872a7b3bd4da10e5fa97a8da86b` and
verifies permitted source patches, configured libraries, ABI headers, Rust archives
and native binaries. Matching filenames do not establish provenance.

```sh
flock --shared "$MEASUREMENT_LOCK" \
  python3 integration/upscaledb/build.py --variant all --jobs 8
flock --shared "$MEASUREMENT_LOCK" \
  python3 integration/upscaledb/build.py --variant test_hooks --jobs 8 --resume
flock --exclusive "$MEASUREMENT_LOCK" prlimit --core=0 \
  python3 integration/upscaledb/correctness.py --bin-dir .worktree/upscaledb
```

`all` builds standard primary/profile variants, not test hooks. No `hetero_*`
variants are supported. The default build root is `.worktree/upscaledb`;
`--output-root` selects another. Pre-existing artifacts require `--resume`;
`--harness-only` also requires verified libraries. Do not relabel old binaries as
current builds or bypass source identity checks.

The correctness churn gate starts 128 workers. Do not inherit an eight-CPU
measurement partition for that gate: the historical CFL-local run exceeded the
unchanged 120-second watchdog there and passed with 128 logical CPUs available.
This is an observed oversubscription limitation, not a universal liveness bound.
Test-hook children intentionally abort; disable core dumps as above.

### Controls and experiments

- **Native:** original upstream public operation and original environment mutex.
- **Refactored:** extracted-body public API retaining its original mutex; separates
  extraction from alternative synchronization.
- **Bridge mutex:** same synchronous adapter with the original borrowed environment
  mutex; separates adapter overhead from alternative lock scheduling.
- **FC/FC-PQ and conventional backends:** same request body and database configuration.
  CFL-local is a local adaptation, not a verified author release. ShflLock is
  excluded. CLH retains raw queue nodes for the short-lived process.

| Question | Entry points |
|---|---|
| Fixed-work / sustained single-DB requests | `upscaledb/run_trials.py`, `evaluate.py`, `analyze.py`, `overview.py` |
| Physical-core / NUMA / SMT placement | `upscaledb/scaling.py`, `scaling_report.py`, `dimension_report.py` |
| Finder/inserter roles | `upscaledb/hypothesis_roles.py` |
| Reservation mechanism | `upscaledb/hypothesis_reservation.py` |
| Artificial intermittent clients | `upscaledb/hypothesis_bursts.py` |
| Controlled callback costs | `upscaledb/boundary_cost.py` |
| Synthetic independent arrivals | `upscaledb/boundary_arrival.py` |
| Single-DB working set / worker count | `upscaledb/boundary_database.py` |

Each controller exposes `--help` and records its configuration and provenance.
Workload parameters must match across backends within each comparison. Artificial
sleep, Poisson arrivals and callback costs are mechanism/sensitivity probes, not
production traces. They do not alter the database's find/insert implementation.
Historical counted-admission experiments are not included: this ABI requires
external quiescence and does not implement concurrent stop/drain.

## redb: complete write transactions

`integration/redb` is a separate Cargo workspace using the shared `libdlock` path
dependency and **unmodified redb 3.1.0**, pinned by its Cargo lockfile. Native uses
redb's single-writer boundary directly; Mutex/MCS/FC/FC-PQ wrap the complete
transaction without moving a transaction guard across threads. The client
scheduler remains an experimental treatment, not an unchanged production workload.

The workload covers eight synchronous clients, all-one-record transactions and
four small/four 8- or 64-record transactions, with Immediate and None durability.
Transaction sizes and durability match across backends within a cohort.

Choose eight available CPUs and the appropriate memory node, replacing the example:

```sh
export CPUS=8,9,10,11,12,13,14,15
flock --shared "$MEASUREMENT_LOCK" taskset -c "$CPUS" \
  python3 integration/redb_transactions.py --prepare-only \
  --output-root .worktree/redb-campaign --cpus "$CPUS" --numa-node 0
flock --exclusive "$MEASUREMENT_LOCK" taskset -c "$CPUS" \
  python3 integration/redb_transactions.py --smoke \
  --output-root .worktree/redb-campaign
flock --exclusive "$MEASUREMENT_LOCK" taskset -c "$CPUS" \
  python3 integration/redb_transactions.py --run \
  --output-root .worktree/redb-campaign
flock --shared "$MEASUREMENT_LOCK" \
  python3 integration/redb_transactions.py --analyze-only \
  --output-root .worktree/redb-campaign
```

Preparation records CPU order and memory policy; subsequent modes enforce them.
Use `--numa-node none` only when intentionally omitting binding and report that
qualification. `--target-root` defaults to `.worktree/redb-cargo-target`;
`--build-jobs` controls compilation concurrency. Smoke covers twenty real DB cells,
separate from the 180-cell formal matrix; analysis requires the full formal cohort.
The runner checks contents and close/reopen, records DB hashes and removes DB files
while retaining result/error records. Capacity guards remain 8 million records per
worker and a 512 MiB file limit, not silent truncation.

## Interpreting evidence

Primary builds measure throughput/CPU/request latency. Profile builds separately
measure service allocation and may perturb scheduling. Service-wall Jain is not
CPU fairness or completed-count fairness. Histogram percentiles are bucket upper
bounds; pooled p99 is not a per-client progress bound. Duration runs can change the
completed operation mix; tx/s, records/s and fixed-work speedup differ.

Immediate and None durability are separate experiments. Close/reopen is not a
power-loss test; None is not a durable-commit promise. Preserve manifests, commands,
source snapshots and raw success/failure records together. No new performance
claim follows merely from restoring or validating this setup.
