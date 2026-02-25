# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Role

You are a Senior Research Scientist specializing in Concurrent Systems and Distributed Synchronization.

## Project Overview

Research project implementing **usage-fair delegation locks** in Rust, targeting PPoPP 2027 (Aug 2026) / EuroSys 2027 (Oct 2026). The key contribution is FC-PQ (Flat Combining with Priority Queue), which achieves an O(C_max) fairness bound while maintaining throughput close to unfair delegation — breaking the traditional fairness-performance tradeoff.

**Core thesis:** Delegation locks decouple fairness from data locality. Shared data stays in the combiner's L1 regardless of serving order, so reordering for fairness is essentially free — unlike traditional locks (CFL, MCS) where fair handoff forces cross-core cache migration.

## Build

All Rust code lives under `rust/`. Requires nightly Rust, GCC/Clang, x86_64. Always use `--release` (debug builds are extremely slow for concurrent code). See [rust/README.md](rust/README.md) for full CLI reference.

## Research Context

| Document | Purpose |
|----------|---------|
| [RESEARCH_PLAN.md](RESEARCH_PLAN.md) | Thesis, contributions, positioning vs CFL/Syncord/TCLocks/U-SCL, evaluation plan, paper outline |
| [TODO.md](TODO.md) | Phased roadmap: metrics, tradeoff validation, combiner study, baselines, applications, writing |
| [STATUS_REPORT.md](STATUS_REPORT.md) | Known bugs, hard blockers, algorithm improvements, venue strategy |

## Architecture Specs

| Component | Spec |
|-----------|------|
| Binary crate (`dlock`): CLI, benchmarks | [rust/README.md](rust/README.md) |
| Library crate (`libdlock`): locks, traits, tests | [rust/lib-dlock/README.md](rust/lib-dlock/README.md) |
| DLock2 (function-delegate locks, primary API) | [rust/lib-dlock/src/dlock2/README.md](rust/lib-dlock/src/dlock2/README.md) |
| DLock1 (callback-based locks, legacy API) | [rust/lib-dlock/src/dlock/README.md](rust/lib-dlock/src/dlock/README.md) |
| Parker (thread waiting strategies) | [rust/lib-dlock/src/parker/README.md](rust/lib-dlock/src/parker/README.md) |
| Benchmark harness | [rust/src/benchmark/README.md](rust/src/benchmark/README.md) |
| C reference implementations | [c/README.md](c/README.md) |

## CI

GitHub Actions (`.github/workflows/rust.yml`): nightly Rust, GCC + Clang, mold linker. Runs `rustfmt`, `cargo build --release`, `cargo test --release` on push/PR to main.
