#!/usr/bin/env python3
"""Write a dogfood interaction deployment binding file.

The binding file contains local paths and environment-variable names only. Token
values stay in the process environment and are never written or printed here.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
from typing import Any


def string_array_from_json(raw: str, field: str) -> list[str]:
    if not raw.strip():
        return []
    try:
        value = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise SystemExit(f"{field} must be a JSON array of strings: {exc}") from exc
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        raise SystemExit(f"{field} must be a JSON array of strings")
    return value


def env_allowlist(raw: str) -> list[str]:
    stripped = raw.strip()
    if not stripped:
        return []
    if stripped.startswith("["):
        return string_array_from_json(stripped, "--responder-env-allowlist")
    return [item.strip() for item in stripped.split(",") if item.strip()]


def positive_int(raw: str, field: str) -> int | None:
    stripped = raw.strip()
    if not stripped:
        return None
    try:
        value = int(stripped)
    except ValueError as exc:
        raise SystemExit(f"{field} must be a positive integer") from exc
    if value <= 0:
        raise SystemExit(f"{field} must be a positive integer")
    return value


def validate_repo(raw: str) -> None:
    parts = raw.split("/")
    if len(parts) != 2 or not all(parts):
        raise SystemExit(f"--repo must be owner/name, got {raw!r}")


def binding_json(args: argparse.Namespace) -> dict[str, Any]:
    validate_repo(args.repo)
    responder: dict[str, Any] = {
        "command": args.responder_command,
        "args": string_array_from_json(args.responder_args_json, "--responder-args-json"),
        "env_allowlist": env_allowlist(args.responder_env_allowlist),
    }
    if args.responder_cwd.strip():
        responder["cwd"] = args.responder_cwd
    timeout = positive_int(args.responder_timeout_secs, "--responder-timeout-secs")
    if timeout is not None:
        responder["timeout_secs"] = timeout

    return {
        "forge": {"base_url": args.base_url},
        "repository": args.repo,
        "default_profile": args.profile_id,
        "profiles": {
            args.profile_id: {
                "human_token_env": args.human_token_env,
                "agent_token_env": args.agent_token_env,
            }
        },
        "responders": {args.responder_id: responder},
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", required=True)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--profile-id", required=True)
    parser.add_argument("--responder-id", required=True)
    parser.add_argument("--responder-command", required=True)
    parser.add_argument("--responder-args-json", default="[]")
    parser.add_argument("--responder-env-allowlist", default="")
    parser.add_argument("--responder-cwd", default="")
    parser.add_argument("--responder-timeout-secs", default="")
    parser.add_argument("--human-token-env", default="TEMPER_INTERACTION_HUMAN_TOKEN")
    parser.add_argument("--agent-token-env", default="TEMPER_INTERACTION_AGENT_TOKEN")
    args = parser.parse_args()

    data = binding_json(args)
    out = Path(os.path.expanduser(args.out))
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n")
    os.chmod(out, 0o600)
    print(f"wrote {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
