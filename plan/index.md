# Plan Index

This index highlights the most important active plans first. Dated plan
documents live in subfolders under `plan/YYYY-MM-DD/`.

## Current Priorities

### Priority 1: Publication Analysis Framework

- [Publication Analysis Plan](./2026-09-23/logp-publication-analysis.md)
  - Status: illustrated manuscript and illustration-first HTML explainer complete; empirical evaluation remains separate
  - Delivered: separate performance and fairness models, proofs, family comparisons,
    all 21 DLock2 and 11 DLock1 target mappings, mathematical checks, and a typeset manuscript
  - Scope: analysis artifacts only; production lock implementations unchanged
  - Entry point: [Analysis artifact](../analysis/logp/README.md)
  - Companion: [Fair Service, Local Execution](../analysis/logp/story-draft.tex), usage-fair locks → execution-coupling problem → analysis framework → delegation/FC-PQ; seven pages, three editable SVG concept illustrations, 10 evidence slots; built and visually inspected
  - Reader-first entry: [Who gets the next turn?](../analysis/logp/story-explainer.html), thirteen chapters, fifteen numbered visuals, three teaching controls; includes lock-scheduling background and mechanism-level FIFO/locality/SCL/CFL context before the trilemma
  - Follow-up: HTML motivation now explicitly frames the fairness–performance–work-conservation tension before delegation, without asserting a universal impossibility or implementation guarantee

### Priority 2: Algorithm Direction

- [Algorithm Improvement Plan](./2026-03-23/algorithm-improvement-plan.md)
  - Status: active
  - Focus: replace exact-only fair scheduling with a more practical scheduler
    direction centered on `FC-EW`, combiner budgeting, adaptive fairness, and
    sliced delegation
  - Relevance: it affects both the implementation roadmap and research story;
    use the publication analysis to evaluate its assumptions before changes

### Background: Initial LogP Proposal

- [LogP Analysis Proposal](./2026-03-23/algorithm-improvement-plan.md#logp-analysis-proposal)
  - Updated: 2026-09-23, on `research/logp-analysis`
  - Status: superseded by the approved publication analysis above
  - Focus: source-grounded communication costs, batch-size crossover,
    scoped usage-fairness proof, and a falsifiable calibration plan
  - Boundary: analyzes current FC/FC-PQ and baselines; does not approve FC-EW

## By Date

### 2026-09-23

- [Publication Analysis Plan](./2026-09-23/logp-publication-analysis.md)

- [LogP analysis update to the existing algorithm plan](./2026-03-23/algorithm-improvement-plan.md#logp-analysis-proposal)

### 2026-03-23

- [Algorithm Improvement Plan](./2026-03-23/algorithm-improvement-plan.md)

## Maintenance Rule

When adding a new plan:

1. Put it in a dated folder: `plan/YYYY-MM-DD/`
2. Add it to `By Date`
3. Add it to `Current Priorities` if it is still active and important
