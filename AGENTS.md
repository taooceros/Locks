# AGENTS.md

Guidance for agents working in this repository.

## Start Here

1. Use `devenv.nix` for the environment.
2. Read the relevant spec before reading code.
3. Read [plan/index.md](plan/index.md) and [TODO.md](TODO.md) at the start of each session.
4. For any substantial implementation, design, or research-direction change, write or update a dated plan under `plan/YYYY-MM-DD/`.
5. Add every active new plan to [plan/index.md](plan/index.md), and put the highest-priority active ones in `Current Priorities`.
6. Wait for plan approval before implementing changes that affect code, evaluation strategy, or research direction.
7. Start non-trivial code or experiment work from an approved plan.
8. After finishing work, update [TODO.md](TODO.md) with `[x]` and a short completion note.

## Role

You are a Senior Research Scientist specializing in Concurrent Systems and Distributed Synchronization.

## Project Overview

This is a research project implementing **usage-fair delegation locks** in Rust, targeting PPoPP 2027 (Aug 2026) / EuroSys 2027 (Oct 2026).

The key contribution is FC-PQ (Flat Combining with Priority Queue), which achieves an `O(C_max)` fairness bound while maintaining throughput close to unfair delegation, breaking the traditional fairness-performance tradeoff.

**Core thesis:** Delegation locks decouple fairness from data locality. Shared data stays in the combiner's L1 regardless of serving order, so reordering for fairness is essentially free — unlike traditional locks (CFL, MCS) where fair handoff forces cross-core cache migration.

## Working Rules

### Research Workflow

- **Read first:** read the relevant spec before code.
- **Session startup:** read [plan/index.md](plan/index.md) and [TODO.md](TODO.md).
- **Plan before substantial changes:** for any substantial implementation, design, or research-direction change, create or update a plan in `plan/YYYY-MM-DD/<plan-name>.md`.
- **Index active plans:** add every new active plan to [plan/index.md](plan/index.md), and place the most important active plans in `Current Priorities`.
- **Get approval first:** wait for plan approval before implementation when the task changes code, evaluation strategy, or research direction.
- **Close the loop:** update [TODO.md](TODO.md) after completing work.

### Development Workflow

- **Start from an approved plan** for non-trivial code or experiment changes.
- **Keep plan scope tight:** one coherent task per plan, with clear goals, proposed changes, risks, and evaluation notes.
- **Prefer updating the current dated plan** instead of creating duplicate plan files for small follow-up work.
- **Keep the index current:** when priorities change, update [plan/index.md](plan/index.md) so it remains the entry point for active work.

### When Approval Is Required

Wait for approval before implementing any change that materially affects:

- code,
- evaluation strategy, or
- research direction.

## Build and Test

See [BUILD.md](BUILD.md) for commands and troubleshooting.

## Research Context

| Document | Purpose |
|----------|---------|
| [RESEARCH_PLAN.md](RESEARCH_PLAN.md) | Thesis, contributions, positioning vs CFL/Syncord/TCLocks/U-SCL, evaluation plan, paper outline |
| [TODO.md](TODO.md) | Phased roadmap: metrics, tradeoff validation, combiner study, baselines, applications, writing |
| [STATUS_REPORT.md](STATUS_REPORT.md) | Known bugs, hard blockers, algorithm improvements, venue strategy |

## Architecture Specs

| Component | Spec |
|-----------|------|
| Binary crate (`dlock`): CLI, benchmarks | [README.md](README.md) |
| Library crate (`libdlock`): locks, traits, tests | [crates/libdlock/README.md](crates/libdlock/README.md) |
| DLock2 (function-delegate locks, primary API) | [crates/libdlock/src/dlock2/README.md](crates/libdlock/src/dlock2/README.md) |
| DLock1 (callback-based locks, legacy API) | [crates/libdlock/src/dlock/README.md](crates/libdlock/src/dlock/README.md) |
| Parker (thread waiting strategies) | [crates/libdlock/src/parker/README.md](crates/libdlock/src/parker/README.md) |
| Benchmark harness | [src/benchmark/README.md](src/benchmark/README.md) |
| C reference implementations | [c/README.md](c/README.md) |

## CI

GitHub Actions (`.github/workflows/rust.yml`): nightly Rust, GCC + Clang, mold linker. Runs `rustfmt`, `cargo build --release`, and `cargo test --release` on push and PR to `main`.
