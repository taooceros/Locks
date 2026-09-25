# Database experiment setup

This directory contains the experiment harnesses, not a database mutex replacement
API or a portable performance claim. Run from the repository root on Linux/x86_64.
Use a new output directory for each cohort; keep failed attempts rather than
replacing selected trials. Raw data, databases, native sources, binaries and
analysis output belong under ignored `.worktree/` paths.

## Environment and resource discipline

Install Nix and devenv, initialize the existing C submodule, then enter the pinned
environment:

```sh
git submodule update --init --recursive
devenv shell
```

The environment provides nightly Rust, Clang/libclang, mold, Python, pyarrow,
matplotlib, numactl, Autoconf/Automake/libtool, GNU make, Git and util-linux
(`taskset`, `flock`, `findmnt`, `prlimit`). UpScaleDB's builder obtains pinned
Boost/GCC and upstream source through Nix/Git; its first build requires network
access. `--source-repository` can select an existing local upstream clone, but
the required commit and permitted patches are still verified. redb's Cargo
lockfile pins its standalone dependencies, including redb 3.1.0.

Inspect `lscpu -e=CPU,CORE,SOCKET,NODE` and the current process affinity before
choosing CPUs. CPU numbers and NUMA nodes in historical defaults describe the
original machine, not a universal topology. Use distinct physical cores for a
physical-core experiment; choose SMT/oversubscription deliberately. Memory binding
may be prohibited in containers. An explicitly unbound run is not comparable to a
node-bound cohort without that qualification.

All cooperating experiment worktrees must use **the same** external measurement
lock. Builds/analysis acquire it shared; correctness/smoke and timed runs acquire it
exclusively. Merely choosing disjoint CPU sets does not prevent cache, memory or
power interference, and this convention cannot isolate unrelated host tasks.

```sh
mkdir -p "$HOME/.cache/locks-experiments"
export MEASUREMENT_LOCK="$HOME/.cache/locks-experiments/measurement.lock"
# Wrap build/analysis commands: flock --shared "$MEASUREMENT_LOCK" COMMAND ...
# Wrap correctness/timed commands: flock --exclusive "$MEASUREMENT_LOCK" COMMAND ...
```

## Source-level verification

These checks do not constitute application performance results:

```sh
cargo build --workspace --release --locked
cargo test -p libdlock --release --lib dlock2_unit_test::ownership
cargo test -p upscaledb-bridge --release -- --test-threads=1
cargo test -p upscaledb-bridge --release --features profile,test-hooks -- --test-threads=1
python3 -m unittest discover -s integration/upscaledb -p 'test_*.py' -v
cargo test --manifest-path integration/redb/Cargo.toml --release --locked
```

The bridge tests cover synchronous results, callback exclusion, error propagation,
reentry rejection, multiple handles and join-before-destroy. Ownership tests cover
non-Copy request/result payloads and exact destruction. Native/database correctness
and actual workload smoke remain separate prerequisites.

## UpScaleDB: build and experiment families

The builder pins upstream UpScaleDB to
`cb124e1f91601872a7b3bd4da10e5fa97a8da86b`, verifies permitted source patches,
configured libraries, ABI headers and Rust/native binary identities. A matching
filename alone is not evidence that a library can be reused.

```sh
python3 integration/upscaledb/build.py --variant all --jobs 8
python3 integration/upscaledb/build.py --variant test_hooks --jobs 8 --resume
```

`all` includes standard primary/profile variants, not test hooks or heterogeneous
clients. The latter have separate patched libraries and `hetero_*` binaries.
The default build root is `.worktree/upscaledb`; `--output-root` selects another.
An existing checkout requires `--resume`. `--harness-only` additionally requires a
verified existing library. Do not bypass provenance checks or relabel an old
binary as a build of the current sources.

Run the real database gates after building; the test-hook gate deliberately aborts
child processes, so disable core dumps:

```sh
flock --exclusive "$MEASUREMENT_LOCK" prlimit --core=0 \
  python3 integration/upscaledb/correctness.py --bin-dir .worktree/upscaledb
```

The churn gate starts **128 workers**. Do not inherit an eight-CPU measurement
partition for this check. During integration, CFL-local exceeded the unchanged
120-second watchdog on eight allowed CPUs; the unchanged churn workload completed
with all 128 logical CPUs available. This is an observed oversubscription/progress
limit, not permission to hide a failed gate or claim a universal liveness bound.

Build the heterogeneous-client matrix separately:

```sh
python3 integration/upscaledb/build.py --resume --jobs 8 \
  --variants hetero_native,hetero_bridge_mutex,hetero_fc,hetero_fc_pq,hetero_uscl,hetero_cfl_local,hetero_mcs,hetero_profile,hetero_bridge_mutex_profile,hetero_fc_profile,hetero_fc_pq_profile,hetero_uscl_profile,hetero_cfl_local_profile,hetero_mcs_profile
flock --exclusive "$MEASUREMENT_LOCK" \
  python3 integration/upscaledb/batch_error_gate.py \
  --build-root .worktree/upscaledb --output-root .worktree/upscaledb-batch-gate \
  --variants hetero_native hetero_fc_pq hetero_fc_pq_profile --cpu 20 --memory-node 0
flock --exclusive "$MEASUREMENT_LOCK" numactl --membind=0 \
  .worktree/upscaledb/upscaledb-hetero_fc_pq \
  --mix batch --placement split --cpus 20,21,22,23,24,25,26,27 \
  --seed 101 --preload 64 --seconds 1 --max-records 5000000
```

Replace the example CPU/node placement with available resources. These are error
and runtime checks, not research measurements. Dedicated boundary studies retain
their original topology constraints; changing a filesystem path does not make
their CPU/NUMA design portable. Cost's historical site allocation registry is
optional (`--resource-registry`); supplying it enables its original slot check.

The dedicated preparation modes compile their workload harnesses and run finite
correctness/smoke gates without executing the formal timing matrix:

| Controller | Required historical CPU mask | Memory node | Gate invocations |
|---|---|---|---|
| `boundary_cost.py --prepare-only` | `20-27` | `0` | 22 fixed-count smokes |
| `boundary_tables.py --prepare-only` | `56-63` | `1` | 120 layout/table/backend gates |
| `high_contention.py --prepare-only` | `32-63` | `1` | 132 primary/diagnostic gates |

Use `taskset` and `numactl --membind` with those declared placements on matching
hardware. Each takes `--output-root` and `--binary-root`; tables/contention also
take `--frozen-root`. The unified build root can supply both binary and frozen
inputs. A placement mismatch is an error, not an automatic topology substitution.

| Question | Setup/controller | Analysis |
|---|---|---|
| Fixed-work and sustained DB behavior | `upscaledb/run_trials.py`, `evaluate.py` | `analyze.py`, `overview.py` |
| Physical-core, NUMA and SMT sensitivity | `upscaledb/scaling.py` | `scaling_report.py`, `dimension_report.py` |
| H1: heterogeneous find/insert roles | `upscaledb/hypothesis_roles.py` | controller's analysis mode |
| H2: retained reservation mechanism | `upscaledb/hypothesis_reservation.py` | controller's analysis mode |
| H3: artificial intermittent requests | `upscaledb/hypothesis_bursts.py` | controller's analysis mode |
| Controlled callback costs | `upscaledb/boundary_cost.py` | controller's analysis mode |
| Independent scheduled arrivals | `upscaledb/boundary_arrival.py` | controller's analysis mode |
| DB working set and worker count | `upscaledb/boundary_database.py` | controller's analysis mode |
| Shared/independent environments, ten backends | `upscaledb/boundary_tables.py` | controller's analysis mode |
| Uniform/hot routing under contention | `upscaledb/high_contention.py` | `high_contention_report.py` |
| Pure reads, single inserts, eight-insert batches | `upscaledb/heterogeneous_clients.py` | `heterogeneous_clients_analysis.py` |

Run each controller with `--help` for its prepared/run/analyze phases and resource
arguments. Analysis-only modes consume existing records; they do not recreate
missing measurements. Do not mix smoke with primary trials or pool different
source/binary/placement cohorts. The old counted-admission investigation is
historical: the current bridge has no concurrent-stop/drain API.

### Application and baseline contract

- Restricted in-memory, nontransactional UpScaleDB with fixed-width keys/records;
  not general disk-backed or recovery-enabled integration.
- Whole permitted operation runs under one selected environment gate. FC/FC-PQ
  may execute it on another worker; conventional locks execute on the requester.
- Context and caller-owned output stay alive until synchronous return. No unwind,
  exception, longjmp, cancellation or recursive submission crosses the ABI.
- Join all callers and TLS destructors before exclusive destroy. There is no
  concurrent stop, drain or reclamation guarantee.
- Eight-insert batching is **not a transaction**; an error can leave partial
  success. Native and bridge variants must use the same operation boundary.
- Multiple tables within one environment still share that environment's gate.
  `split` uses independent environments; it does not bypass a required mutex.
- ShflLock is excluded from DB comparisons due to known unsafe atomic/shared-field
  access. CFL-local is a local port, not a certified paper artifact, and retains
  cross-handle accounting state. CLH retains raw queue nodes until process exit;
  this short-process setup is not a long-lived lock-reclamation guarantee.
- USCL retains the original fixed TSC-calibration/slice assumptions and explicit
  equal worker weights. Results are not a reproduction of a modern OS scheduler.

## redb: complete write transactions

`integration/redb` is a separate Cargo workspace using the shared `libdlock` path
dependency. Native uses redb's single-writer boundary directly; Mutex/MCS/FC/FC-PQ
wrap the complete transaction without moving a transaction guard across threads.
The workload covers eight synchronous clients, all-one-record transactions and
four small/four 8- or 64-record transactions, with Immediate and None durability.

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
completed operation mix, so tx/s, records/s and fixed-work speedup differ.

Immediate and None durability are separate experiments. Close/reopen is not a
power-loss test; None is not a durable-commit promise. Artificial sleep and Poisson
arrival streams are controlled inputs, not production traces. API overlap is not
an internal queue-length or cache-migration measurement.

Keep manifests, commands, source snapshots, raw success/failure records and analysis
together outside Git. This PR supplies executable setup and interpretation limits,
not the original machine's result archive or generated research reports.
