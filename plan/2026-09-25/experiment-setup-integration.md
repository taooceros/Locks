# Integrate the experiment setup for a main-targeted PR

Status: single-operation UpScaleDB integration restored and locally verified for PR #46.
The earlier complete removal was the assistant's overbroad interpretation.

## Scope

- Base the integration branch on fetched `origin/main`; do not update main directly.
- Retain FC/FC-PQ ownership repairs, shared lock correctness fixes and redb setup.
- Restore the single-operation UpScaleDB integration, its bridge, Native and
  extracted-body/mutex controls, build provenance and correctness gates.
- Preserve the original find/insert bodies and one-operation critical-section
  boundary. Do not restore batch8 or split-environment/application-restructuring
  experiments; keep their historical worktrees and evidence separately.
- Single-DB workload parameters and synthetic arrival/cost probes are experiment
  inputs, not modifications of database operation semantics; label them accordingly.
- Preserve source worktrees and their uncommitted changes. Import selected snapshots
  with temporary Git indexes; do not stage or commit the original worktrees.
- Keep primary/profile data semantics and backend identifiers unchanged. Existing
  machine-specific CPU/NUMA configurations must be explicit and overridable or
  diagnosed, not silently reinterpreted as portable benchmark defaults.
- Include analysis/report generators, not generated reports, raw measurements,
  databases, native checkouts, binaries, caches or new algorithm experiments.
- Add a concise setup/verification entry point and record known baseline limitations.

## Ownership

The integration owner handles the bridge crate, root configuration and documentation.
Separate workers restore the native build/core adapter and standard single-DB
runner/analysis closure. They do not build, test, format or commit mid-flight.
Original worktrees and ignored historical artifacts remain untouched.

## Verification and delivery

Use the pinned devenv environment. Run the restored Python and bridge checks,
build the standard single-operation variants and exercise real-DB correctness and
small matched workloads. Keep measurements separate from smoke. Update the guide,
completion record and existing PR #46; do not directly change main.

## Verification after restoring integration-only scope

- Restored the original single-operation adapter and bridge, retaining the Native,
  extracted-body/original-mutex and borrowed-mutex controls. Inspected the pinned
  upstream find/insert bodies against the extraction patch; the protected operation
  bodies remain the same. No batch8 extension or multitable restructuring restored.
- Fresh build of all 21 standard primary/profile/control variants plus test hooks:
  22 binaries in `.worktree/upscaledb-single-operations`, without `--resume`.
  The local upstream clone supplied Git source only; libraries were freshly built.
  All 22 binary hashes matched their manifests.
- Python provenance/topology tests: 21 passed. Restored CLI/import surfaces:
  18 passed; requesting the removed heterogeneous build variant was rejected.
- Rust bridge tests: 8 primary and 11 profile/test-hook tests passed. The restored
  root workspace release build with `--locked` passed.
- Real DB: 69 gates passed (three checks for each of 21 standard variants, plus
  six test-hook cases). These cover memory policy, missing/duplicate keys, invalid
  sizes, caller-owned output canaries, exact contents, 128-worker churn, lifecycle,
  expected exceptions and fail-stop behavior. Churn ran with CPUs 0–127 available.
- All 22 variants also completed the same fixed-work smoke: 128 finds + 128 inserts,
  64 preloaded records, seed 101, CPUs 16–23 and memory node 0. Every run reported
  the exact 192 final records, zero value/size/status errors, passing integrity and
  clean DB/environment shutdown.
- Runtime commands/raw outcomes and summary are retained under
  `.worktree/single-operation-verification/`. The throwaway verifier was removed
  after completion. No formal performance matrices rerun. Existing build warnings
  remain visible; redb source and original research worktrees/results are unchanged.

## Historical verification after the overbroad removal (superseded)

The user clarified that integration should remain and application logic should not
be changed to favor the experiment. The following removal was not that request's
intended scope; its verification is retained only as a historical record.

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
