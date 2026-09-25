# Integrate the experiment setup for a main-targeted PR

Status: shared process execution and purpose-based filenames implemented and locally verified for PR #46.
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
- Remove the live dashboard server/page at user request; retain CLI experiment
  runners and offline analysis/report generators.
- Group UpScaleDB into `core/`, `runner/`, `experiments/{scaling,hypotheses,boundaries}/`,
  `reports/` and `tests/`, each with a README. Keep redb's runner beside its workload.
- Use repository-root `python3 -m integration...` commands and update every import,
  subprocess, source-archive and compiler path; no old-path compatibility wrappers.
- Provide one canonical build/check/run/analyze workflow. Do not add a new runner
  abstraction or change workloads, timing policy, randomization or failure retention.
- Per user instruction, do not verify comment/doc-only changes. Check the meaningful
  module/file-path cutover once; do not rerun full experiment matrices.

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

## Folder organization and dashboard removal

- Removed the dashboard server/page. Grouped UpScaleDB into core, runner,
  scaling/hypothesis/boundary experiments, reports and tests, with folder READMEs.
  Moved redb's controller beside its Cargo workload. Updated imports, compiler
  inputs, source/archive identities, subprocess commands and CI test discovery.
- Documented the existing build/check/run/analyze workflow rather than adding a
  runner wrapper. No database-operation, workload, timing or failure-policy changes.
- The moved executable paths warranted focused verification: 21 relocated Python
  tests and 18 module `--help` entrypoints passed. Three controller source inventories
  resolved; five companion C++ translation units passed syntax/include checks.
- Built a fresh FC-PQ variant through `integration.upscaledb.core.build` into
  `.worktree/upscaledb-grouped`, without bypassing provenance or modifying old builds.
  The relocated runner completed 128 finds and 128 inserts from 64 preloaded records,
  ending with exactly 192 records and clean shutdown. The relocated analyzer validated
  its new source archive and excluded zero trials. Evidence:
  `.worktree/upscaledb-grouped-smoke/`.
- Prepared `.worktree/redb-grouped` through `integration.redb.run`; all 20 smoke
  cells exited successfully, verified 1,040 live records and exact close/reopen contents.
  Additional entrypoint/compiler/source-inventory evidence:
  `.worktree/grouped-layout-verification/`. Temporary verification scripts removed.
- No formal performance matrices, full 22-variant rebuild or unchanged Rust tests
  rerun for this organization. Documentation/comment-only edits received no extra
  verification. Existing upstream/compiler warnings remain visible.

## Runner simplification and naming plan

- Extract captured-command and logged-controller execution into
  `runner/process_execution.py`, using functions rather than a runner class hierarchy.
  Migrate the duplicated cost/reservation and role/database controllers. Preserve
  raw evidence, event schemas, timeout values and 20s/30s graceful shutdown policies.
- Keep the standard runner's signal/provenance handling and redb's resource-limited
  execution separate: unifying those policies would require extra configuration
  and obscure meaningful differences.
- Rename study/report files by purpose, removing redundant hypothesis/boundary/report
  prefixes. Update every import, invocation, captured source path and current guide;
  retain saved artifact names and historical evidence rather than relabeling them.
- Verify shared process behavior with real subprocesses, focused database smoke and
  relocated entrypoints. Do not rerun formal matrices or unchanged native builds.

### Runner simplification completion

- Four controllers now directly use `capture_command`, `run_logged` and `utc_now`
  from `runner/process_execution.py`. Their duplicate process loops and local clock
  helpers were removed: 100 fewer controller lines, replaced by a 63-line shared
  module (37 fewer production lines across this extraction).
- Purpose-based filenames replace repeated category prefixes: `role_balance`,
  `reservation_delay`, `intermittent_clients`, `placement`, `operation_cost`,
  `arrival_rate`, `database_load`, `run_scaling`, `comparison_matrix`,
  `analyze_trials`, `campaign_overview`, `scaling`, `dimensions`, `hypotheses`.
  Updated commands, imports, source inventories and guides; no old-path wrappers.
- Preserved existing event fields, timeout values, output artifacts and validators.
  The main runner, file-backed specialized controllers and redb retain their
  different process policies rather than gaining a configurable framework.
- All 28 Python tests passed, including seven real-subprocess regressions for
  launch/nonzero failures, captured output, timeout cleanup and exclusive logs.
  All 14 renamed CLI entrypoints and affected controller source inventories passed.
- Shared capture ran the real FC-PQ error gate successfully. Shared logged execution
  ran a standard fixed-work trial and the renamed analyzer: 128 finds + 128 inserts,
  exactly 192 final records, clean shutdown and zero excluded trials.
  Evidence: `.worktree/runner-refactor-verification/` and
  `.worktree/runner-refactor-smoke/`. Temporary smoke script removed.
- No native rebuild, formal matrix, unchanged redb run or cosmetic-only recheck.
