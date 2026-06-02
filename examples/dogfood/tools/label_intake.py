#!/usr/bin/env python3
"""Dogfood-only intake labeler.

The reference workflow routes newly filed issues only after they carry the
`untriaged` identifying label. This helper labels issues created after the
current run started so an operator can simply file a Forgejo issue.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

WORKFLOW_IDENTIFYING_LABELS = {"untriaged", "epic", "design", "code", "implementation"}
NON_WORKFLOW_SKIP_LABELS = {"product"}
INTAKE_SKIP_LABELS = WORKFLOW_IDENTIFYING_LABELS | NON_WORKFLOW_SKIP_LABELS

DECISION_IGNORE = "ignore"
DECISION_LABEL = "label"
DECISION_SKIP = "skip"


def parse_time(raw: str) -> datetime:
    value = raw.replace("Z", "+00:00")
    parsed = datetime.fromisoformat(value)
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed.astimezone(timezone.utc)


class Forgejo:
    def __init__(self, base_url: str, token: str) -> None:
        self.base = base_url.rstrip("/")
        self.token = token

    def api(self, method: str, path: str, data: dict[str, Any] | None = None) -> Any:
        body = None
        headers = {
            "Authorization": f"token {self.token}",
            "Accept": "application/json",
        }
        if data is not None:
            body = json.dumps(data).encode()
            headers["Content-Type"] = "application/json"
        req = urllib.request.Request(
            self.base + "/api/v1" + path,
            data=body,
            headers=headers,
            method=method,
        )
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                payload = resp.read()
                return json.loads(payload.decode()) if payload else None
        except urllib.error.HTTPError as exc:
            snippet = exc.read(300).decode(errors="replace").strip()
            raise RuntimeError(f"{method} {path} failed ({exc.code}): {snippet}") from None


def quote(value: str) -> str:
    return urllib.parse.quote(value, safe="")


def label_names(issue: dict[str, Any]) -> set[str]:
    labels = issue.get("labels")
    if not isinstance(labels, list):
        return set()
    names = set()
    for label in labels:
        if isinstance(label, dict) and isinstance(label.get("name"), str):
            names.add(label["name"])
    return names


def intake_label_decision(issue: dict[str, Any], started_at: datetime, seen: set[int]) -> str:
    """Return whether a Forgejo issue should be labeled as workflow intake."""

    if issue.get("pull_request") is not None:
        return DECISION_IGNORE
    number = issue.get("number")
    if not isinstance(number, int):
        return DECISION_IGNORE
    created_raw = issue.get("created_at")
    if not isinstance(created_raw, str):
        return DECISION_IGNORE
    if parse_time(created_raw) < started_at:
        return DECISION_IGNORE
    labels = label_names(issue)
    if labels & INTAKE_SKIP_LABELS:
        return DECISION_SKIP
    if number in seen:
        return DECISION_IGNORE
    return DECISION_LABEL


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--owner", required=True)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--started-at", required=True)
    parser.add_argument("--stop-file", required=True)
    parser.add_argument("--poll-secs", type=float, default=5.0)
    args = parser.parse_args()

    token = os.environ.get("TEMPER_FORGEJO_TOKEN", "").strip()
    if not token:
        raise SystemExit("TEMPER_FORGEJO_TOKEN is required")

    started_at = parse_time(args.started_at)
    stop_file = Path(args.stop_file)
    forgejo = Forgejo(args.base_url, token)
    owner = quote(args.owner)
    repo = quote(args.repo)
    seen: set[int] = set()

    print(f"intake-labeler: watching {args.owner}/{args.repo} since {started_at.isoformat()}", flush=True)
    while not stop_file.exists():
        try:
            page = 1
            while True:
                issues = forgejo.api(
                    "GET",
                    f"/repos/{owner}/{repo}/issues?state=open&page={page}&limit=50",
                )
                if not isinstance(issues, list) or not issues:
                    break
                for issue in issues:
                    if not isinstance(issue, dict):
                        continue
                    number = issue.get("number")
                    decision = intake_label_decision(issue, started_at, seen)
                    if decision == DECISION_SKIP:
                        if isinstance(number, int):
                            seen.add(number)
                        continue
                    if decision != DECISION_LABEL or not isinstance(number, int):
                        continue
                    forgejo.api("POST", f"/repos/{owner}/{repo}/issues/{number}/labels", {"labels": ["untriaged"]})
                    seen.add(number)
                    print(f"intake-labeler: labeled issue #{number} untriaged", flush=True)
                if len(issues) < 50:
                    break
                page += 1
        except Exception as exc:
            print(f"intake-labeler: retrying after error: {exc}", file=sys.stderr, flush=True)
        time.sleep(args.poll_secs)
    print("intake-labeler: stop file observed", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
