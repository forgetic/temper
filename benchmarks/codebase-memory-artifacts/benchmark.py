#!/usr/bin/env python3
"""Opt-in native graph-artifact benchmark; no command runs by default."""
import argparse
import copy
import json
import os
import shutil
import sys
from pathlib import Path

from contract import Refusal, digest, validate_config
from source import inspect_source, write_json


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    fixture = sub.add_parser("fixture", help="synthetic correctness workflow; never native performance evidence")
    fixture.add_argument("--output", type=Path, required=True)
    inspect = sub.add_parser("inspect-source", help="read-only source and controlled clone ignore identity")
    inspect.add_argument("--repository", type=Path, required=True)
    inspect.add_argument("--commit", required=True)
    validate = sub.add_parser("validate-config", help="validate frozen configuration without launching provider")
    validate.add_argument("--config", type=Path, required=True)
    run = sub.add_parser("run", help="explicit Linux root opt-in; requires a frozen config checksum")
    run.add_argument("--config", type=Path, required=True)
    run.add_argument("--frozen-sha256", required=True)
    run.add_argument("--state-root", type=Path, required=True)
    run.add_argument("--output", type=Path, required=True)
    cleanup = sub.add_parser("cleanup", help="remove only verified owned state after dedicated-account exit")
    cleanup.add_argument("--state-root", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "fixture":
            from fixture import fixture_report
            if args.output.exists():
                raise Refusal("output-already-exists")
            write_json(args.output, fixture_report())
        elif args.command == "inspect-source":
            print(json.dumps(inspect_source(args.repository, args.commit), sort_keys=True))
        elif args.command == "cleanup":
            cleanup_state(args.state_root)
        else:
            contents = args.config.read_bytes()
            config = validate_config(json.loads(contents))
            if args.command == "validate-config":
                print(json.dumps({"valid": True, "sha256": digest(contents)}, sort_keys=True))
                return 0
            if digest(contents) != args.frozen_sha256:
                raise Refusal("frozen-config-checksum-mismatch")
            if args.output.exists():
                raise Refusal("output-already-exists")
            executable = shutil.which(config["provider"]["path"])
            if not executable:
                raise Refusal("provider-executable-not-found")
            runtime_config = copy.deepcopy(config)
            runtime_config["provider"]["path"] = str(Path(executable).resolve())
            runtime_config["source"]["repository"] = str(Path(config["source"]["repository"]).resolve())
            from native import run_native
            report = run_native(runtime_config, args.state_root.resolve(), args.frozen_sha256,
                                lambda message: print(message, file=sys.stderr, flush=True), contents)
            write_json(args.output, report)
        return 0
    except Refusal as error:
        # Exception reasons are closed codes, never raw provider/source content.
        print("benchmark refused: " + str(error), file=sys.stderr)
        return 2
    except (KeyError, TypeError, ValueError):
        print("benchmark refused: malformed-configuration-or-evidence", file=sys.stderr)
        return 2


def cleanup_state(path):
    from accounting import require_unused
    if os.geteuid() != 0 or path.is_symlink() or not path.is_absolute():
        raise Refusal("cleanup-requires-owned-absolute-state")
    marker = path / "ownership.json"
    if marker.is_symlink():
        raise Refusal("cleanup-marker-invalid")
    data = json.loads(marker.read_text())
    stat = path.stat()
    if data.get("kind") != "temper-artifact-benchmark-v1" or data.get("root") != str(path.resolve()):
        raise Refusal("cleanup-marker-invalid")
    if (data.get("device"), data.get("inode"), data.get("uid")) != (stat.st_dev, stat.st_ino, stat.st_uid):
        raise Refusal("cleanup-ownership-mismatch")
    require_unused(data["uid"])
    shutil.rmtree(path)


if __name__ == "__main__":
    sys.exit(main())
