# redb experiment setup

This directory contains a client-side transaction workload using **unmodified
redb 3.1.0**, pinned by `integration/redb/Cargo.lock`. It does not patch database
internals or replace an internal database mutex. The modified-UpScaleDB setup was
removed from PR #46 at the user's request; historical worktrees/results remain
separate and are not prerequisites for this setup.

The client scheduler is still an experimental treatment, not a claim of a
transparent lock replacement or a production workload. Native uses redb's
single-writer boundary directly; Mutex/MCS/FC/FC-PQ wrap complete transactions.
Transaction guards do not move across threads. Within each cohort, all backends
use the same transaction sizes and durability mode.

Run from the repository root on Linux/x86_64. Use a new output directory for each
cohort; preserve failed attempts. Raw data, databases, binaries and analysis output
belong under ignored `.worktree/` paths.

## Environment and resource discipline

Install Nix and devenv, initialize the existing lock-library C submodule, then
enter the pinned environment:

```sh
git submodule update --init --recursive
devenv shell
```

The environment provides nightly Rust, Clang/libclang, mold, Python, pyarrow,
matplotlib, numactl, Git and util-linux (`taskset`, `flock`, `findmnt`). Initial
setup and uncached dependency downloads require network access.

Inspect `lscpu -e=CPU,CORE,SOCKET,NODE` and the current process affinity before
choosing CPUs. Historical CPU numbers are not universal topology defaults. Choose
distinct physical cores for a physical-core experiment; choose SMT or
oversubscription deliberately. Memory binding may be prohibited in containers.
An unbound run is not comparable to a node-bound cohort without that qualification.

All cooperating experiment worktrees must use the same external measurement lock.
Builds/analysis acquire it shared; correctness/smoke and timed runs acquire it
exclusively. Disjoint CPU sets do not prevent cache, memory or power interference;
this convention cannot isolate unrelated host tasks.

```sh
mkdir -p "$HOME/.cache/locks-experiments"
export MEASUREMENT_LOCK="$HOME/.cache/locks-experiments/measurement.lock"
# Builds/analysis: flock --shared "$MEASUREMENT_LOCK" COMMAND ...
# Correctness/timed runs: flock --exclusive "$MEASUREMENT_LOCK" COMMAND ...
```

## Source-level verification

These checks are not application performance results:

```sh
cargo build --workspace --release --locked
cargo test -p libdlock --release --lib dlock2_unit_test::ownership
cargo build --manifest-path integration/redb/Cargo.toml --release --locked
cargo build --manifest-path integration/redb/Cargo.toml --release --locked --features redb_profile
```

Ownership tests cover non-Copy request/result payloads and exact destruction.
Actual database smoke remains a separate prerequisite.

## Complete write transactions

`integration/redb` is a separate Cargo workspace using the shared `libdlock` path
dependency. The workload covers eight synchronous clients, all-one-record
transactions and four small/four 8- or 64-record transactions, with Immediate and
None durability.

Choose **eight available CPUs**, replacing the example below as needed. Preparation
records the exact CPU order and memory policy; subsequent modes enforce them.

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

Use `--numa-node none` during preparation only when intentionally omitting memory
binding; report that changed placement. `--target-root` defaults to the local
`.worktree/redb-cargo-target`; `--build-jobs` controls compilation concurrency.
Smoke covers twenty real DB cells and is separate from the 180-cell formal matrix.
Analysis requires the complete formal cohort. The runner records filesystem and
source/binary identity, verifies database contents/close-reopen, hashes and removes
trial DB files while preserving result/error records. Capacity guards remain
8 million records per worker and a 512 MiB file limit, not silent truncation.

## Interpreting and preserving evidence

Primary builds measure throughput/CPU/request latency; profile builds separately
measure service allocation and may perturb scheduling. Service-wall Jain is not
CPU fairness or completed-count fairness. Histogram percentiles are bucket upper
bounds; pooled p99 is not a per-client progress bound. Duration runs can change the
completed transaction mix, so tx/s, records/s and fixed-work speedup differ.

Immediate and None durability are separate experiments. Close/reopen is not a
power-loss test; None is not a durable-commit promise. These constructed clients
are not a production trace or an existing application's unchanged workload.

Keep manifests, commands, source snapshots, raw success/failure records and analysis
together outside Git. This PR supplies executable setup and interpretation limits,
not the original machine's result archive or generated research reports.
