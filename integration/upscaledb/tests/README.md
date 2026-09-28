# UpScaleDB Python tests

`test_build.py` checks library reuse, build provenance, and bridge-header ABI rules. `test_scaling.py` checks topology and NUMA input handling, CPU selection, runner commands, and scaling-case resume evidence. `test_process_execution.py` uses real subprocesses to check retained failure output, deadlines, graceful cleanup and exclusive logs. None builds the native harness or runs benchmark matrices.

From the repository root:

```sh
python3 -m unittest discover -s integration/upscaledb/tests -p 'test_*.py' -v
```

Real-DB self-tests of built variants are invoked separately with `python3 -m integration.upscaledb.core.correctness`.
