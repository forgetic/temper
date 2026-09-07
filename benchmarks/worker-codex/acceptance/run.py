#!/usr/bin/env python3
"""Run host-owned behavioral tests against a candidate delivery-policy checkout."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repository", type=Path)
    parser.add_argument("--json-output", type=Path)
    parser.add_argument("--timeout-seconds", type=int, default=120)
    args = parser.parse_args()
    repository = args.repository.resolve(strict=True)
    if not (repository / "Cargo.toml").is_file():
        parser.error("repository must contain Cargo.toml")
    if args.timeout_seconds <= 0:
        parser.error("--timeout-seconds must be positive")

    with tempfile.TemporaryDirectory(prefix="worker-codex-acceptance-") as temporary:
        root = Path(temporary)
        tests = root / "tests"
        tests.mkdir()
        for name in ("parser", "evaluate", "report", "compatibility"):
            shutil.copyfile(Path(__file__).parent / f"{name}.rs", tests / f"{name}.rs")
        manifest = (
            '[package]\nname = "worker-codex-acceptance"\n'
            'version = "0.0.0"\nedition = "2024"\n\n'
            '[dependencies]\ndelivery-policy = { path = '
            + json.dumps(str(repository)) + ' }\n\n[workspace]\n'
        )
        (root / "Cargo.toml").write_text(manifest)
        environment = os.environ.copy()
        environment["CARGO_TARGET_DIR"] = str(root / "target")
        command = ["cargo", "test", "--offline", "--quiet", "--manifest-path", str(root / "Cargo.toml")]
        started = time.monotonic()
        try:
            result = subprocess.run(command, cwd=root, env=environment, capture_output=True,
                                    text=True, timeout=args.timeout_seconds, check=False)
            code, stdout, stderr = result.returncode, result.stdout, result.stderr
        except subprocess.TimeoutExpired as error:
            code = 124
            stdout = (error.stdout or b"").decode(errors="replace")
            stderr = (error.stderr or b"").decode(errors="replace") + "\nAcceptance timed out.\n"
        report = {
            "schema_version": 1,
            "passed": code == 0,
            "exit_code": code,
            "duration_seconds": time.monotonic() - started,
            "stdout": stdout,
            "stderr": stderr,
        }
        if args.json_output:
            args.json_output.parent.mkdir(parents=True, exist_ok=True)
            args.json_output.write_text(json.dumps(report, indent=2) + "\n")
        print(stdout, end="")
        if stderr:
            print(stderr, end="", file=sys.stderr)
        return 0 if code == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
