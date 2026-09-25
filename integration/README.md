# Database experiment integrations

Choose a database, then follow its standard workflow. The specialized studies and
report generators are optional; they are not additional setup steps.

| Start here | Purpose |
|---|---|
| [UpScaleDB](upscaledb/README.md) | Original single-operation bodies with alternative synchronization |
| [redb](redb/README.md) | Complete transactions using the unmodified redb library |

```text
integration/
├── upscaledb/
│   ├── core/          Build, synchronization adapter, native harness, correctness gate
│   ├── runner/        Standard fresh-process trial runner
│   ├── experiments/   Optional scaling, hypothesis and boundary studies
│   ├── reports/       Offline analysis and report generation
│   └── tests/         Python build/provenance/topology tests
└── redb/              Transaction workload, pinned Cargo workspace and runner
```

Each directory has its own README. Run Python tools **from the repository root**
using `python3 -m integration...`; do not run their file paths directly. Cross-folder
imports and subprocesses use the same module convention. No compatibility wrappers
or live dashboard are included.

## Shared environment

```sh
git submodule update --init --recursive
devenv shell
```

The pinned environment supplies Rust/C/C++ build tools, Python, plotting packages
and Linux placement tools. Initial setup and uncached dependency downloads require
network access. See [BUILD.md](../BUILD.md) for library checks.

Inspect `lscpu -e=CPU,CORE,SOCKET,NODE` and the process affinity before choosing CPUs.
Examples use illustrative CPU/node IDs, not universal defaults. Use distinct
physical cores for physical-core experiments; choose SMT/oversubscription explicitly.
Memory binding may be unavailable in containers; qualify any unbound run.

All cooperating worktrees must use **the same measurement lock path**. Set it once
in the shell before using the database-specific recipes:

```sh
mkdir -p "$HOME/.cache/locks-experiments"
export MEASUREMENT_LOCK="$HOME/.cache/locks-experiments/measurement.lock"
```

Builds and analysis acquire it shared; correctness/smoke and timed runs acquire it
exclusively. Disjoint CPUs do not prevent shared cache, memory or power interference.
This coordination does not isolate unrelated host tasks.

## Output and interpretation

Use a fresh result directory per cohort. Builds, downloaded sources, databases,
binaries, source snapshots and raw results belong under ignored `.worktree/` paths.
Do not overwrite failures or relabel an old binary as a current build.

Keep primary measurements separate from profiling: instrumentation can alter
scheduling. Service-wall fairness is not CPU fairness or completed-count fairness;
histogram percentiles are bucket upper bounds, not per-client progress bounds.
Smoke verifies setup/correctness, not performance. Synthetic studies are not
production traces. Folder organization does not change workload or database logic.
