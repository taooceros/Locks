# UpScaleDB core

`build.py` builds pinned native, refactored, bridge, profile, and test-hook variants. It verifies the source and binary provenance recorded in `build-VARIANT.json`. `correctness.py` runs the built binaries' real-DB self-tests. `native_harness.cc`, `bridge.h`, `private_ops.h`, `bridge-ops.patch`, and `native-lock-timing.patch` are the compiler/patch inputs; keep them together when changing the build.

From the repository root:

```sh
python3 -m integration.upscaledb.core.build --variants native,refactored,fc,test_hooks --jobs 8
python3 -m integration.upscaledb.core.correctness --variants native refactored fc
```

Builds and manifests default to `.worktree/upscaledb/`; correctness looks there unless `--bin-dir` is supplied and always exercises `test_hooks` as well as the selected variants. Existing source checkouts require `--resume`; reusing a separate verified database build requires `--resume --harness-only --reuse-build-root` and a different `--output-root`. Builds require the pinned upstream checkout/toolchain and are not performed by the runner.
