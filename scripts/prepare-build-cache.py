#!/usr/bin/env python3
"""Invalidate retained Cargo artifacts once after a kache upgrade."""

import json
import os
from pathlib import Path
import re
import shutil
import subprocess


def prepare():
    wrapper = os.environ.get("RUSTC_WRAPPER")
    # An explicit alternate/disabled wrapper is a supported local override.
    if wrapper is not None and Path(wrapper).name != "kache":
        return
    binary = shutil.which(wrapper or "kache")
    if binary is None:
        return
    version = subprocess.check_output([binary, "--version"], text=True).strip()
    parsed = re.fullmatch(r"kache (\d+)\.(\d+)\.(\d+)(?:[-+].*)?", version)
    if parsed is None or tuple(map(int, parsed.groups())) < (0, 26, 3):
        raise SystemExit("Shared build caching requires kache >= 0.26.3; upgrade the wrapper and daemon.")
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version=1"], text=True
    ))
    target = Path(metadata["target_directory"])
    marker = target / ".temper-kache-version"
    previous = marker.read_text().strip() if marker.is_file() else None
    if previous == version:
        return
    if target.exists():
        print(f"Refreshing Cargo artifacts for {version} (previous: {previous or 'unrecorded'}).", flush=True)
        subprocess.run(["cargo", "clean", "--target-dir", str(target)], check=True)
    target.mkdir(parents=True, exist_ok=True)
    marker.write_text(version + "\n")


if __name__ == "__main__":
    prepare()
