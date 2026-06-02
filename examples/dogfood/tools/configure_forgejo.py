#!/usr/bin/env python3
"""Idempotently prepare the live Forgejo repo for dogfooding.

The admin token is read from DOGFOOD_ADMIN_TOKEN. It is never printed.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import shlex
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

EVENTS = [
    "push",
    "issues",
    "issue_comment",
    "pull_request",
    "pull_request_review_approved",
    "pull_request_review_rejected",
    "pull_request_review_comment",
]

PRODUCT_LABEL_NAME = "product"
PRODUCT_LABEL_COLOR = "5319e7"
PRODUCT_LABEL_DESCRIPTION = "Product discussion and planning records that are not workflow intake."


def load_env(path: Path) -> dict[str, str]:
    env: dict[str, str] = {}
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        try:
            parts = shlex.split(line, posix=True)
        except ValueError as exc:
            raise SystemExit(f"failed to parse {path}: {exc}") from exc
        if len(parts) != 1 or "=" not in parts[0]:
            continue
        key, value = parts[0].split("=", 1)
        env[key] = value
    return env


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
                if not payload:
                    return None
                return json.loads(payload.decode())
        except urllib.error.HTTPError as exc:
            snippet = exc.read(300).decode(errors="replace").strip()
            raise RuntimeError(f"{method} {path} failed ({exc.code}): {snippet}") from None


def quote(value: str) -> str:
    return urllib.parse.quote(value, safe="")


def content_sha(forgejo: Forgejo, owner: str, repo: str, path: str, branch: str) -> str | None:
    encoded_path = "/".join(quote(part) for part in path.split("/"))
    try:
        data = forgejo.api("GET", f"/repos/{owner}/{repo}/contents/{encoded_path}?ref={quote(branch)}")
    except RuntimeError as exc:
        if "failed (404)" in str(exc):
            return None
        raise
    if isinstance(data, dict) and isinstance(data.get("sha"), str):
        return data["sha"]
    raise RuntimeError(f"contents response for {path} did not contain sha")


def commit_file(
    forgejo: Forgejo,
    owner: str,
    repo: str,
    path: str,
    contents: str,
    message: str,
    branch: str,
) -> None:
    encoded_path = "/".join(quote(part) for part in path.split("/"))
    sha = content_sha(forgejo, owner, repo, path, branch)
    payload: dict[str, Any] = {
        "branch": branch,
        "content": base64.b64encode(contents.encode()).decode(),
        "message": message,
    }
    if sha:
        payload["sha"] = sha
        forgejo.api("PUT", f"/repos/{owner}/{repo}/contents/{encoded_path}", payload)
    else:
        forgejo.api("POST", f"/repos/{owner}/{repo}/contents/{encoded_path}", payload)


def hook_url(hook: dict[str, Any]) -> str | None:
    config = hook.get("config")
    if isinstance(config, dict) and isinstance(config.get("url"), str):
        return config["url"]
    url = hook.get("url")
    return url if isinstance(url, str) else None


def list_repo_labels(forgejo: Forgejo, owner: str, repo: str) -> list[dict[str, Any]]:
    labels: list[dict[str, Any]] = []
    page = 1
    while True:
        payload = forgejo.api("GET", f"/repos/{owner}/{repo}/labels?page={page}&limit=50")
        if not isinstance(payload, list):
            raise RuntimeError("list labels returned a non-list response")
        labels.extend(label for label in payload if isinstance(label, dict))
        if len(payload) < 50:
            return labels
        page += 1


def validate_ci_args(install_ci: bool, ci_workflow_file: str | None) -> None:
    if ci_workflow_file and not install_ci:
        raise SystemExit("--ci-workflow-file requires explicit --install-ci")
    if install_ci and not ci_workflow_file:
        raise SystemExit("--install-ci requires --ci-workflow-file")


def ensure_repo_label(forgejo: Forgejo, owner: str, repo: str, name: str, color: str, description: str) -> None:
    labels = list_repo_labels(forgejo, owner, repo)
    existing = next((label for label in labels if label.get("name") == name), None)
    payload = {"name": name, "color": color, "description": description}
    if existing is None:
        forgejo.api("POST", f"/repos/{owner}/{repo}/labels", payload)
        print(f"label created: {name}")
        return

    existing_color = str(existing.get("color", "")).lstrip("#").lower()
    desired_color = color.lstrip("#").lower()
    needs_patch = existing_color != desired_color or existing.get("description") != description
    if needs_patch:
        label_id = existing.get("id")
        if label_id is None:
            raise RuntimeError(f"existing label {name!r} did not contain id")
        forgejo.api("PATCH", f"/repos/{owner}/{repo}/labels/{label_id}", payload)
        print(f"label updated: {name}")
    else:
        print(f"label ensured: {name}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--owner", required=True)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--roles-env", required=True)
    parser.add_argument("--webhook-url")
    parser.add_argument("--webhook-secret-file")
    parser.add_argument("--ci-workflow-file")
    parser.add_argument("--install-ci", action="store_true")
    parser.add_argument("--default-branch", default="main")
    parser.add_argument("--permission", default="write")
    args = parser.parse_args()

    token = os.environ.get("DOGFOOD_ADMIN_TOKEN", "").strip()
    if not token:
        raise SystemExit("DOGFOOD_ADMIN_TOKEN is required")

    env = load_env(Path(args.roles_env))
    users = env.get("DOGFOOD_PERMISSION_USERS", "").split()
    forgejo = Forgejo(args.base_url, token)
    owner = quote(args.owner)
    repo = quote(args.repo)

    forgejo.api("GET", f"/repos/{owner}/{repo}")
    print(f"repo found: {args.owner}/{args.repo}")
    ensure_repo_label(
        forgejo,
        owner,
        repo,
        PRODUCT_LABEL_NAME,
        PRODUCT_LABEL_COLOR,
        PRODUCT_LABEL_DESCRIPTION,
    )

    validate_ci_args(args.install_ci, args.ci_workflow_file)
    if args.install_ci:
        workflow = Path(args.ci_workflow_file).read_text()
        forgejo.api("PATCH", f"/repos/{owner}/{repo}", {"has_actions": True})
        commit_file(
            forgejo,
            owner,
            repo,
            ".forgejo/workflows/ci.yml",
            workflow,
            "dogfood: install Forgejo Actions CI",
            args.default_branch,
        )
        print(f"CI workflow ensured on {args.default_branch}: .forgejo/workflows/ci.yml")

    for user in users:
        forgejo.api(
            "PUT",
            f"/repos/{owner}/{repo}/collaborators/{quote(user)}",
            {"permission": args.permission},
        )
        print(f"permission ensured: {user} -> {args.permission}")

    if args.webhook_url:
        if not args.webhook_secret_file:
            raise SystemExit("--webhook-url requires --webhook-secret-file")
        secret = Path(args.webhook_secret_file).read_text().strip()
        payload = {
            "type": "gitea",
            "active": True,
            "events": EVENTS,
            "config": {
                "url": args.webhook_url,
                "content_type": "json",
                "secret": secret,
            },
        }
        hooks = forgejo.api("GET", f"/repos/{owner}/{repo}/hooks")
        if not isinstance(hooks, list):
            raise RuntimeError("list hooks returned a non-list response")
        existing = next((hook for hook in hooks if hook_url(hook) == args.webhook_url), None)
        if existing and existing.get("id") is not None:
            patch_payload = dict(payload)
            patch_payload.pop("type", None)
            forgejo.api("PATCH", f"/repos/{owner}/{repo}/hooks/{existing['id']}", patch_payload)
            print(f"webhook updated: {args.webhook_url}")
        else:
            forgejo.api("POST", f"/repos/{owner}/{repo}/hooks", payload)
            print(f"webhook registered: {args.webhook_url}")

    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:  # keep token out of tracebacks/logs
        print(f"configure_forgejo.py: error: {exc}", file=sys.stderr)
        raise SystemExit(1)
