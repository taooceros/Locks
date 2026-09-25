"""Repository paths shared by the grouped UpScaleDB tools."""
from pathlib import Path

UPSCALEDB = Path(__file__).resolve().parent
ROOT = UPSCALEDB.parents[1]
CORE = UPSCALEDB / 'core'
RUNNER = UPSCALEDB / 'runner'
EXPERIMENTS = UPSCALEDB / 'experiments'
REPORTS = UPSCALEDB / 'reports'
TESTS = UPSCALEDB / 'tests'
