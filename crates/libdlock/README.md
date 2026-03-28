# lib-dlock/ — Library Crate (`libdlock`)

All lock implementations, traits, and tests. This is the core of the project.

## Module Map

```
src/
├── lib.rs                      # Feature gates, compile_error for non-x86_64
├── dlock/                      # DLock1: callback-based API (see dlock/README.md)
├── dlock2/                     # DLock2: function-delegate API (see dlock2/README.md)
├── parker/                     # Thread parking abstraction (see parker/README.md)
├── spin_lock.rs                # RawSpinLock (implements lock_api::RawMutex)
├── u_scl.rs                    # U-SCL fairlock wrapper (C FFI)
├── c_binding/                  # C reference lock wrappers
│   ├── ccsynch.rs              #   CCCSynch<T, F, I>
│   └── flatcombining.rs        #   CFlatCombining<T, F, I>
├── sequential_priority_queue.rs # SequentialPriorityQueue trait (BTreeSet, BinaryHeap impls)
├── atomic_extension.rs         # AtomicExtension trait (load_acquire, store_release helpers)
├── syncptr.rs                  # SyncMutPtr<T> — raw pointer newtype implementing Send+Sync
├── unit_test.rs                # DLock1 multi-threaded correctness tests
└── dlock2_unit_test.rs         # DLock2 multi-threaded correctness tests
```

## Nightly Features

Declared in `lib.rs`:
- `sync_unsafe_cell` — `SyncUnsafeCell<T>` (thread-safe interior mutability)
- `pointer_is_aligned` — alignment checks on raw pointers
- `type_alias_impl_trait` — `type Foo = impl Trait` syntax
- `thread_id_value` — `ThreadId::as_u64()` for tie-breaking in PQ
- `trait_alias` — `trait Foo = Bar + Baz` syntax

## C Code Integration

`build.rs` compiles C implementations from `../../c/` using the `cc` crate and generates FFI bindings via `bindgen`. Compiled sources:
- `c/CCsynch/ccsynch.c`
- `c/FlatCombining/original/flatcombining.c`
- `c/u-scl/fairlock.c`

## Running Tests

```bash
# All tests (always use --release for concurrent code)
cargo test --release --verbose

# Single DLock2 test
cargo test -p libdlock --release --lib dlock2_unit_test::fc_pq_btree::threads_4 -- --nocapture

# Single DLock1 test
cargo test -p libdlock --release --lib unit_test::fc_test -- --nocapture
```

Tests spawn 2/4/8 threads, each performing 1000 lock operations (50 for serial/spin-heavy variants). Assertion: final counter == threads * iterations. 60-second timeout per test.

## Feature Flags

- `combiner_stat` (default, enabled): Tracks per-thread combining time via `__rdtscp`. Adds `combiner_time_stat` field to nodes and `get_combine_time()` to the DLock2 trait.
