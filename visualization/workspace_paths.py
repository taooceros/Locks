#!/usr/bin/env python3

"""Helpers for resolving worktree-local and legacy data paths."""

from __future__ import annotations

import os
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
WORKTREE_DIR = REPO_ROOT / ".worktree"
DEFAULT_OUTPUT_DIR = WORKTREE_DIR / "output"
DEFAULT_PROFILE_DIR = WORKTREE_DIR / "profiles"
LEGACY_OUTPUT_DIR = REPO_ROOT / "visualization" / "output"


def resolve_output_dir() -> Path:
    override = os.environ.get("DLOCK_OUTPUT_DIR")
    if override:
        return Path(override).expanduser().resolve()

    if any(DEFAULT_OUTPUT_DIR.iterdir()) if DEFAULT_OUTPUT_DIR.is_dir() else False:
        return DEFAULT_OUTPUT_DIR

    if LEGACY_OUTPUT_DIR.exists():
        return LEGACY_OUTPUT_DIR

    return DEFAULT_OUTPUT_DIR


def resolve_profile_dir() -> Path:
    override = os.environ.get("DLOCK_PROFILE_DIR")
    if override:
        return Path(override).expanduser().resolve()

    return DEFAULT_PROFILE_DIR
