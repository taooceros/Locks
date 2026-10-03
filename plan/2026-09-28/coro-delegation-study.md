# Coroutine delegation-lock fairness study (2026-09-28)

Status: exploratory study complete (PR #52, branch `coro/delegation-study`).
A side project, separate from the thesis plan in the root `README.md`; it
changes no production lock in `crates/libdlock`.

Design and hypotheses: [RESEARCH.md](../../crates/coro_delegation/RESEARCH.md).
Results: [FINDINGS.md](../../crates/coro_delegation/FINDINGS.md).

## Goal

Measure the fairness effects that a service-time Jain index over lock clients
misses when a delegation lock runs inside a cooperative work-stealing
executor:

- F1 combiner burden: which worker threads pay for other tasks' critical sections;
- F2 bystander delay: latency for tasks that share the combiner's worker;
- F3 service fairness under 1:8 critical-section cost heterogeneity.

Hypotheses H-A (burden concentrates), H-B (bystander p99 exceeds one pass),
H-C (usage-ordered FC-PQ reaches service Jain >= 0.95 at >= 0.9x FIFO
throughput), H-D (mitigations restore burden Jain >= 0.9).

## Changes

- `crates/coro_delegation`: a pinned work-stealing executor with per-worker
  stats, the `dispatch`, `ces`, `fc` and `fcpq` locks plus H-D knobs and wake
  placement options, the `coro-bench` binary, and run/summary scripts.
- `crates/coro_tokio_baseline`: the same workload on tokio 1.53
  (`tokio::sync::Mutex`, `async-lock`, `std`/`parking_lot` mutexes) as a
  cross-runtime baseline.
- Both crates are workspace default members, so CI builds and tests them.

## Risks

- The executor is a model, not tokio. The tokio baseline bounds this risk:
  coro `dispatch` lands at 0.90-1.05x `tokio::sync::Mutex`.
- Measurement noise from concurrent jobs and turbo carry-over. Runs held the
  shared measurement lock and cooled down between runs. The two affected sets
  are archived as superseded (`results/*.tar.zst`).
- Result corpus in Git: the 950 final JSONs (19 MB) stay tracked as evidence.
  This is an exception to the `.worktree/` policy, recorded in RESEARCH.md.

## Evaluation

Each run has 3 repeats and reports the median [min, max]. Tables come from
`scripts/summarize.py` and `scripts/summarize_tokio.py`. Outcome:

- H-A holds for CES (burden Jain = 1/W). It is refuted for FC/FC-PQ under
  executor balancing.
- H-B is refuted under balancing. Without balancing, the effect is
  starvation, not delay.
- H-C is refuted on fairness (best service Jain 0.935, `fcpq-h16-home`). It
  holds on ops cost but fails on work cost (-16 % lock utilisation).
- H-D holds for CES with `ces-k64-home`: burden Jain 0.993-0.999, at
  0.95-0.99x `ces` throughput.
- Against tokio, `ces-k64-home` / `fc-remote` keep a sustained margin of
  1.5-1.6x. The bursty margin is 1.2-1.6x once tokio's LIFO slot is
  discounted.
