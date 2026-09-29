# Finding 003: Eight Physical Cores, Eight Workers, UpScaleDB

**Date:** 2026-09-26
**Status:** Summarised from existing data (joined scaling study); no new runs
**Component:** UpScaleDB integration, all five backends (Native, FC, FC-PQ, USCL, CFL-local)
**Source:** `experiment/upscaledb-fc-pq-integration@dcbfacb:docs/reports/upscaledb-joined-dimensions/report.md`; raw `.worktree/upscaledb-joined-scaling/overview/scaling.csv` (git-ignored on the run machine)
**Setup:** n=3 fresh processes per cell; 8 workers pinned to 8 distinct physical cores on the same socket

## Observation

### Fixed work: 400K find + 400K insert, 8 workers

| Backend | Duration | Throughput | CPU-s | Paired speedup vs Native [range] |
|---|---:|---:|---:|---:|
| Native | 1.822 s | 440K op/s | 9.23 | 1.00x |
| FC | 1.055 s | 758K op/s | 8.43 | 1.73x [1.63, 1.80] |
| FC-PQ | 1.206 s | 663K op/s | 8.42 | 1.51x [1.41, 1.60] |
| USCL | 1.061 s | 754K op/s | 2.07 | 1.72x [1.63, 1.80] |
| CFL-local | 0.987 s | 848K op/s | 7.88 | 1.91x [1.61, 2.38] (wide) |

### Duration 10 s: 4 finders + 4 inserters

| Backend | find/s | insert/s | CPU-s / 10 s |
|---|---:|---:|---:|
| Native | 284K | 298K | 38.9 |
| FC | 603K | 607K | 80.0 |
| FC-PQ | 844K | 521K | 80.0 |
| USCL | 362K | 255K | 19.6 |
| CFL-local | 604K | 604K | 80.0 |

### SMT: 8 hyperthreads on 4 cores, fixed work

Paired speedup vs native: FC 2.17x, FC-PQ 1.99x, USCL 2.13x.

## Interpretation

- With one physical core per worker (threads <= CPUs, the rule from finding
  002), delegation is 1.7x (FC) / 1.5x (FC-PQ) over the native mutex
  in fixed work and about 2x in sustained load.
- The native mutex itself degrades from 645K op/s (4W) to 440K op/s (8W);
  part of the relative delegation advantage is native getting worse, not
  delegation getting better.
- USCL matches FC throughput in fixed work at roughly one quarter of the
  CPU-s (2.07 vs 8.43). The CPU-s gap is a spin-policy artifact (DLock2
  waiters spin, U-SCL waiters block; see finding 002), not a delegation
  cost. Under sustained load USCL falls to about half of FC (362K/255K vs
  603K/607K).
- FC-PQ is 13% below FC at identical CPU-s (8.42 vs 8.43). This run predates
  `fcpq_fast_path`, which does not close the saturated gap: E0(b)
  ([PR #48](https://github.com/taooceros/Locks/pull/48)) still measures 32W
  FC-PQ/FC 0.65 at cs 1 and 0.88 at cs 1000, and the fast path changes
  saturated per-op cost by only 2-5 ns. [INFERENCE] The gap is saturated per-op
  serial cost, not a scheduling effect.
- FC-PQ shifts the sustained mix toward finds (844K find/s vs 521K
  insert/s, against FC's 603K/607K): fairness by request selection changes
  which work completes, not just how much.

## Caveats

- n=3 per cell; the bracketed ranges are trial min/max, not confidence
  intervals.
- CFL-local is an unverified proxy with a wide range [1.61, 2.38]; it is not
  Park/Eom's CFL artifact.
- Fixed-work and duration rows are different modes and not comparable with
  each other; duration changes the completed mix.
