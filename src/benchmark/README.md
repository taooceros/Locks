# benchmark/ — Benchmark Harness

Orchestrates lock performance experiments: thread spawning, CPU pinning, warmup, measurement, and result output.

## Architecture

```
benchmark/
├── bencher.rs              # Bencher struct: config + dispatch (dlock1 vs dlock2)
├── dlock.rs                # DLock1 benchmark dispatch
├── dlock2.rs               # DLock2 benchmark dispatch
├── dlock2/
│   ├── proportional_counter.rs  # Counter with configurable CS length
│   ├── fetch_and_multiply.rs    # Multiply on shared counter
│   ├── queue.rs                 # Enqueue/dequeue workload
│   └── priority_queue.rs        # Insert/extract-min workload
├── records.rs              # Records struct + Arrow IPC writer
├── helper.rs               # File creation utilities
└── old_records.rs          # Legacy CSV output (unused)
```

## Execution Flow

1. **`Bencher::benchmark()`** dispatches to `benchmark_dlock1` or `benchmark_dlock2`
2. Workload function iterates over lock targets, constructing each lock via `DLock2Target::to_locktype()`
3. **`start_benchmark()`** (in each workload):
   - Spawns `num_thread` threads, each pinned to a core via `core_affinity`
   - Warmup phase: threads run but `warmup_done` is false, so stats are not accumulated
   - Measurement phase: threads accumulate loop counts, latencies, hold times
   - `stop_signal` fires after `duration` seconds; threads join and return `Records`
4. **`finish_benchmark()`**: Computes JFI (Jain's Fairness Index), writes Arrow IPC

## Records Schema

Per-thread output record fields:
- `id`, `cpu_id`, `thread_num`, `cpu_num`, `trial`
- `loop_count`, `num_acquire` — throughput metrics
- `cs_length`, `non_cs_length` — workload configuration
- `duration` — measurement duration in seconds
- `combiner_latency`, `waiter_latency` — per-op latency vectors (if `--stat-response-time`)
- `hold_time` — total TSC cycles spent in critical section
- `combine_time` — total TSC cycles spent combining (if `combiner_stat` feature)
- `jfi`, `normalized_share` — fairness metrics computed post-hoc
- `locktype`, `waiter_type` — identifying strings

## Output Format

Arrow IPC `.arrow` files in `<output_path>/<lock_name>/`. Writers are kept open across batches via thread-local `WriterMap` and `finish()`ed on drop.
