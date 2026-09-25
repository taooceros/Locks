# Integrate the experiment setup for a main-targeted PR

Status: source integration and local verification complete; delivery through a
user-requested PR to main, not a direct update of main.

## Scope

- Base the integration branch on fetched `origin/main`; do not update main directly.
- Bring in the tested FC/FC-PQ ownership repair, conventional-backend prerequisites,
  synchronous UpScaleDB bridge, pinned native build and correctness gates.
- Consolidate the scaling, hypotheses, boundary/multitable, high-contention and
  heterogeneous-client harnesses, plus the standalone redb transaction experiment.
- Preserve source worktrees and their uncommitted changes. Import selected snapshots
  with temporary Git indexes; do not stage or commit the original worktrees.
- Keep primary/profile data semantics and backend identifiers unchanged. Existing
  machine-specific CPU/NUMA configurations must be explicit and overridable or
  diagnosed, not silently reinterpreted as portable benchmark defaults.
- Include analysis/report generators, not generated reports, raw measurements,
  databases, native checkouts, binaries, caches or new algorithm experiments.
- Add a concise setup/verification entry point and record known baseline limitations.

## Ownership

The integration owner handles the shared Rust/C bridge and root configuration/docs.
One worker owns `integration/upscaledb/` except `bridge.h`; another owns
`integration/redb/` and `integration/redb_transactions.py`. No worker commits,
pushes, builds, runs tests or formats mid-flight. Run integrated verification once
all edits are complete; rerun only a failed path after a concrete correction.

## Verification and delivery

Use the pinned devenv environment. Run existing focused Python and Rust release
checks, then real small workloads exercising the newly combined native/bridge,
heterogeneous batch and redb primary/profile paths. Do not rerun publication
performance matrices. Preserve failures and distinguish build/runtime checks from
research measurements. Review the staged file inventory for source-only scope,
commit the integration branch, push it and open a PR against main.

## Integration findings and exercised checks

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
