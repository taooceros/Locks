# Integrate the experiment setup for a main-targeted PR

Status: removal complete and locally verified; PR #46 retains redb with its
unmodified database dependency and shared lock fixes, not modified-UpScaleDB setup.

## Scope

- Base the integration branch on fetched `origin/main`; do not update main directly.
- Retain the tested FC/FC-PQ ownership repair, shared lock correctness fixes,
  and standalone redb transaction experiment.
- Remove `integration/upscaledb/`, its Rust/C bridge crate, workspace membership,
  bridge-only dependencies and CI steps, and obsolete setup instructions.
- Preserve source worktrees and their uncommitted changes. Import selected snapshots
  with temporary Git indexes; do not stage or commit the original worktrees.
- Keep primary/profile data semantics and backend identifiers unchanged. Existing
  machine-specific CPU/NUMA configurations must be explicit and overridable or
  diagnosed, not silently reinterpreted as portable benchmark defaults.
- Include analysis/report generators, not generated reports, raw measurements,
  databases, native checkouts, binaries, caches or new algorithm experiments.
- Add a concise setup/verification entry point and record known baseline limitations.

## Ownership

The integration owner performs this removal on `integration/experiment-setup`.
Original experimental worktrees, ignored artifacts and historical results remain
untouched. The initial integration used separate UpScaleDB and redb workers;
their original verification is retained below as historical evidence only.

## Verification and delivery

Use the pinned devenv environment. Build the reduced root workspace and prepare
both redb variants, then run its twenty-cell real-DB smoke. Do not rerun the formal
performance matrix. Update the setup guide, completion record and PR description,
commit and push to the existing main-targeted PR without directly changing main.

## Verification after user-requested removal

- Removed all tracked `integration/upscaledb/` files and `crates/upscaledb-bridge`,
  the workspace/lockfile entry, bridge CI steps, and UpScaleDB-only Autotools
  prerequisites. Replaced the setup guide with the retained redb workflow.
- `cargo build --workspace --release --locked -j 4` passed in devenv after removal.
  Existing compiler/dependency warnings remain; no warning-free claim.
- Fresh redb preparation built primary and profile variants with locked
  dependencies, using CPUs 16–23 and memory node 0.
- The real-DB smoke passed all 20 cells (five backends × two durability modes ×
  primary/profile). Every cell verified exactly 1,040 live records and exact
  close/reopen contents. This is correctness evidence, not a performance cohort.
- Evidence: `.worktree/redb-without-upscaledb/`. No formal timing matrix rerun;
  original worktrees, results and ignored historical build artifacts untouched.

## Historical integration findings (before UpScaleDB removal)

- The root release workspace build passed with the repository's existing compiler
  warnings and a dependency future-incompatibility warning; not a warning-free build.
- Existing Python provenance/topology/analysis checks: 26 passed.
- FC and both FC-PQ adapters' non-Copy identity/drop checks: 3 passed.
- Bridge release tests: 8 primary and 11 profile+test-hook tests passed, including
  multi-handle exclusion, worker churn, reentry and fail-stop behavior.
- Fifteen setup/controller `--help` entry points executed successfully. This checks
  CLI/import availability only, not the full experiment matrices.
- redb integration smoke exposed two integration mistakes before publication:
  argparse attempted to convert the string default `unset` as a NUMA node, and
  CPU parameterization omitted the worker's second startup barrier. The latter
  panicked on the uninitialized clock and left the controller at its second barrier.
  Restored the original two-barrier protocol and used an absent optional argument
  rather than an invalid typed default. Failed roots remain under
  `.worktree/redb-integration-smoke` and `.worktree/redb-integration-verified`;
  corrected preparation uses `.worktree/redb-integration-final`.
- Corrected redb smoke passed all 20 cells: five backends, primary/profile,
  Immediate/None. Each cell verified exactly 1,040 live records, all requested
  CPUs 16–23, and exact close/reopen contents. No formal redb trial was run.
- Sealed priority-queue compile-fail doctest: 1 passed. Root workspace rustfmt
  check passed after formatting only the imported bridge test module.
- A fresh native multi-variant build exposed that the resume guards also rejected
  sources created earlier in the same invocation. The Rust archive guard had the
  same defect. Track this invocation's source kinds and archives separately;
  pre-existing artifacts still require explicit resume and full provenance checks.
  The original failed build root and a cancelled intermediate attempt remain in
  `.worktree/upscaledb` and `.worktree/upscaledb-verified`. The fresh complete
  36-variant build passed in `.worktree/upscaledb-complete`, without `--resume`.
  All 36 distinct build manifests and binary hashes were checked. Its local clone
  source was the upstream checkout freshly downloaded during the first attempt;
  no original experiment worktree or prebuilt database library was borrowed.
- The staged inventory was reviewed: source/configuration/tests/docs only; no raw
  measurements, generated research reports, native checkouts or binaries.
- All 21 standard native/bridge variants passed memory-policy, error/data and
  128-worker churn gates; the test-hook build passed six lifecycle/exception/
  fail-stop cases. These are correctness checks, not timed experiment trials.
  The initial eight-CPU invocation timed out on CFL-local's 128-worker churn
  after 120 seconds. The unchanged CFL-local workload and remaining variants
  passed with all 128 logical CPUs available. This observed oversubscription
  limit remains disclosed in the guide; no watchdog or algorithm was weakened.
- Heterogeneous real-DB smoke: 84/84 passed, spanning all seven backends,
  primary/profile, three mixes and shared/split environments. Used CPUs 16–23,
  memory node 0, one-second smoke windows, 64 preloaded records and the existing
  20-million-record maximum. No formal heterogeneous cohort was run.
- Batch partial-error/record ownership gates: 14/14 primary/profile variants passed.
- Controlled cost preparation: 22 fixed-count smokes passed on CPUs 20–27/node 0;
  its 66 planned primary/diagnostic trials were not executed.
- Multitable preparation: 120 gates passed on its declared CPUs 56–63/node 1.
  The initial invocation on CPUs 32–39 was rejected before workload execution;
  `.worktree/upscaledb-tables-smoke` retains that preparation attempt.
- High-contention preparation: 132 primary/diagnostic smoke gates passed on
  CPUs 32–63/node 1. Its formal 450+27 timing matrix was not executed.
- Successful runtime evidence is retained in `.worktree/upscaledb-heterogeneous-smoke`,
  `.worktree/upscaledb-batch-gate`, `.worktree/upscaledb-cost-smoke`,
  `.worktree/upscaledb-tables-verified` and `.worktree/upscaledb-contention-smoke`.
  Upstream Autotools/C++ warnings are retained, not suppressed. No new publication
  measurements or regenerated research conclusions are included.
