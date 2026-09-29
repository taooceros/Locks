# Plan Index

This index highlights the most important active plans first. Dated plan
documents live in subfolders under `plan/YYYY-MM-DD/`.

## Current Priorities

### Research plan (2026-09-26)

- [README.md](../README.md)
  - Status: current research plan
  - Scope: service-fair delegation thesis, claims H1-H5, experiments E0-E4,
    success and kill criteria; checklist in [TODO.md](../TODO.md)

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
- 2026-09-28: [FC-SL lost-request fix](./2026-09-28/fcsl-lost-request-fix.md)
- 2026-09-28: [Coroutine delegation-lock fairness study](./2026-09-28/coro-delegation-study.md) (side project, not thesis)

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
