#!/usr/bin/env python3
"""Export the companion's editable SVG figures as vector PDFs using librsvg."""

import argparse
from pathlib import Path
import shutil
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--converter",
        default="rsvg-convert",
        help="rsvg-convert executable (default: find it on PATH)",
    )
    args = parser.parse_args()
    converter = shutil.which(args.converter)
    if converter is None:
        parser.error("Install librsvg's rsvg-convert or pass --converter /path/to/rsvg-convert")

    source_dir = Path(__file__).resolve().parent / "figures"
    root = source_dir.parents[2]
    output_dir = root / ".worktree" / "logp" / "figures"
    output_dir.mkdir(parents=True, exist_ok=True)
    for source in sorted(source_dir.glob("*.svg")):
        destination = output_dir / source.with_suffix(".pdf").name
        subprocess.run(
            [converter, "--format", "pdf", "--output", str(destination), str(source)],
            check=True,
        )
        print(f"{source.name} -> {destination.relative_to(root)}")


if __name__ == "__main__":
    main()
