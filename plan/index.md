# Plan Index

This index highlights the most important active plans first. Dated plan
documents live in subfolders under `plan/YYYY-MM-DD/`.

## Current Priorities

### Research plan (2026-09-26)

- [README.md](../README.md)
  - Status: current research plan
  - Scope: service-fair delegation thesis, claims H1-H5, experiments E0-E4,
    success and kill criteria; checklist in [TODO.md](../TODO.md)

### E0(b) FC-PQ low-contention fast path (2026-09-27)

- [FC-PQ fast path](./2026-09-27/e0b-fcpq-fast-path.md)
  - Status: approved as an ablation (workspace `e0b-fastpath`); implemented
    behind default-off `libdlock` features `fcpq_cached_tid`,
    `fcpq_fast_path`, `fcpq_fast_path_notime` and `fcpq_fast_path_stat`, with
    stress and enrollment-window tests; benchmarks pending
  - Scope: FC-PQ only (`crates/libdlock/src/dlock2/fc_pq/`); target
    FC-PQ/FC >= 0.95 at 1 worker

### redb write path: service-time fairness rerun (2026-09-28)

- [redb internal-lock rerun](./2026-09-28/redb-internal-rerun.md)
  - Status: implemented and smoke-verified (workspace `redb-internal`): service
    time per client, `None` primary, 1/2/4/8-client sweep, `uscl` variant,
    FC-PQ with `fcpq_fast_path`; gate 64/64, 165 upstream tests, 336-cell smoke,
    168-cell perf cache-counter cohort and 504-cell formal matrix (0 failures);
    absolute tx/s at 4-8 clients frequency-confounded (turbo vs base per core)
  - Scope: `integration/redb/**`
- [redb perf-02: clock-normalised counters](./2026-09-28/redb-perf-02-clock.md) — done: ref_tsc + per-client clock; FC/FC-PQ two levels = clock; perf overhead ≈ 1 %; fixed 3.0 GHz rerun: FC-PQ/U-SCL 2.4-3.4× → 0.92-1.27×; S2 (C6 off) open

### FC-PQ pass-length (H) ablation in redb (2026-09-29)

- [FC-PQ pass-length ablation](./2026-09-29/fcpq-pass-length-ablation.md)
  - Status: active (workspace `fcpq-h-ablation`, bookmark `experiment/fcpq-pass-length`)
  - Scope: tests whether combiner tenure (FC-PQ's per-pass pop cap H = 64) explains
    FC-PQ's redb advantage over FC; variants `fc_pq_hn` (H = active) and `fc_pq_h8`
    (H = 8) next to `fc_pq` (H = 64, unchanged)

### redb closure-style delegated write API (implemented 2026-09-29)

- [redb closure write API](./2026-09-29/redb-closure-write-api.md)
  - Scope: replace the fixed-insert body with `write(|tx: &mut WriteTransaction| …)`
    run by the delegation lock; panics re-raised on the requester; transfer cohort;
    the rerun harness above (service time, client sweep, `uscl`, FC-PQ fast path,
    perf cohort, power setups) ported onto the closure body
  - Status: gate, upstream tests and smoke pass; parity note (LTO) in the plan outcome

### Theoretical analysis (formal version of the thesis)

- [Publication Analysis Plan](./2026-09-23/logp-publication-analysis.md)
  - Status: separate performance and fairness analyses, proofs, family
    comparisons, eight deterministic checks and a typeset manuscript complete;
    all evidence slots remain open until E1-E3 in the research plan run
  - Entry point: [Analysis artifact](../analysis/logp/README.md)
  - Narrative: [Fair Service, Local Execution](../analysis/logp/story-draft.tex)
    (service sequence vs executor sequence, conditional CFL crossover)
  - Reader-first: [Who gets the next turn?](../analysis/logp/story-explainer.html)
  - Scope: analysis artifacts only; production lock implementations unchanged

### Experiment setup integration

- [Experiment setup integration](./2026-09-25/experiment-setup-integration.md)
  - Status: grouped tools, shared execution helpers and purpose-based names verified for PR #46.
  - Scope: synchronization adapters without application-logic changes, redb setup,
    and shared lock fixes; preserve original worktrees and exclude result artifacts.

### Archived: Algorithm Direction

- [Algorithm Improvement Plan](../docs/archive/algorithm-improvement-plan-2026-03-23.md)
  - Status: archived (pre-thesis); superseded by the research plan in `README.md`
  - Focus: `FC-EW`, combiner budgeting, adaptive fairness, and sliced delegation
  - Its appended [LogP analysis proposal](../docs/archive/algorithm-improvement-plan-2026-03-23.md#logp-analysis-proposal)
    (2026-09-23) is superseded by the publication analysis above

## By Date
- 2026-09-29: [redb closure write API](./2026-09-29/redb-closure-write-api.md)
- 2026-09-29: [FC-PQ pass-length ablation](./2026-09-29/fcpq-pass-length-ablation.md)
- 2026-09-28: [FC-SL lost-request fix](./2026-09-28/fcsl-lost-request-fix.md)

### 2026-09-28

- [redb internal-lock rerun](./2026-09-28/redb-internal-rerun.md)
- [redb perf-02 clock normalisation](./2026-09-28/redb-perf-02-clock.md)

### 2026-09-27

- [E0(b) FC-PQ fast path](./2026-09-27/e0b-fcpq-fast-path.md)

### 2026-09-26

- [Research plan](../README.md) (thesis, experiments E0-E4)

### 2026-09-25

- [Experiment setup integration](./2026-09-25/experiment-setup-integration.md)

### 2026-09-23

- [Publication Analysis Plan](./2026-09-23/logp-publication-analysis.md)
- [LogP analysis update to the algorithm plan](../docs/archive/algorithm-improvement-plan-2026-03-23.md#logp-analysis-proposal) (archived)

### 2026-03-23

- [Algorithm Improvement Plan](../docs/archive/algorithm-improvement-plan-2026-03-23.md) (archived)

## Maintenance Rule

When adding a new plan:

1. Put it in a dated folder: `plan/YYYY-MM-DD/`
2. Add it to `By Date`
3. Add it to `Current Priorities` if it is still active and important
