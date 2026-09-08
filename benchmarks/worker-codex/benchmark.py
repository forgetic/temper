#!/usr/bin/env python3
"""Compare real Temper delivery with Codex on a frozen coding task."""

import argparse
from pathlib import Path
import shutil

from campaign import execute


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repository", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--task-revision", required=True)
    parser.add_argument("--temper-bin", type=Path, required=True)
    parser.add_argument("--fixture-bin", type=Path, required=True)
    parser.add_argument("--analyzer-bin", type=Path, required=True)
    parser.add_argument("--auth-file", type=Path, required=True)
    parser.add_argument("--codex-bin", type=Path, default=shutil.which("codex"))
    parser.add_argument("--mcp-bin", type=Path, default=shutil.which("codebase-memory-mcp"))
    parser.add_argument("--pairs", type=int, default=5)
    parser.add_argument("--timeout-seconds", type=int, default=1800)
    options = parser.parse_args()
    if options.pairs < 1 or options.timeout_seconds < 1:
        parser.error("pairs and timeout must be positive")
    for name in ["temper_bin", "fixture_bin", "analyzer_bin", "auth_file", "codex_bin", "mcp_bin"]:
        path = getattr(options, name)
        if path is None:
            parser.error(f"--{name.replace('_', '-')} is required when absent from PATH")
        setattr(options, name, Path(path).resolve())
    result = execute(options)
    return 0 if result["performance_target_met"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
