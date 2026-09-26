# Plan Index

This index highlights the most important active plans first. Dated plan
documents live in subfolders under `plan/YYYY-MM-DD/`.

## Current Priorities

### Research plan (2026-09-26)

- [README.md](../README.md)
  - Status: current research plan
  - Scope: service-fair delegation thesis, claims H1-H5, experiments E0-E4,
    success and kill criteria; checklist in [TODO.md](../TODO.md)

### Experiment setup integration

- [Experiment setup integration](./2026-09-25/experiment-setup-integration.md)
  - Status: grouped tools, shared execution helpers and purpose-based names verified for PR #46.
  - Scope: synchronization adapters without application-logic changes, redb setup,
    and shared lock fixes; preserve original worktrees and exclude result artifacts.

### Archived: Algorithm Direction

- [Algorithm Improvement Plan](../docs/archive/algorithm-improvement-plan-2026-03-23.md)
  - Status: archived (pre-thesis); superseded by the research plan in `README.md`
  - Focus: `FC-EW`, combiner budgeting, adaptive fairness, and sliced delegation

## By Date

### 2026-09-26

- [Research plan](../README.md) (thesis, experiments E0-E4)

### 2026-09-25

- [Experiment setup integration](./2026-09-25/experiment-setup-integration.md)

### 2026-03-23

- [Algorithm Improvement Plan](../docs/archive/algorithm-improvement-plan-2026-03-23.md) (archived)

## Maintenance Rule

When adding a new plan:

1. Put it in a dated folder: `plan/YYYY-MM-DD/`
2. Add it to `By Date`
3. Add it to `Current Priorities` if it is still active and important
