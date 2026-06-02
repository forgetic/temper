#!/usr/bin/env python3
"""Dogfood engineer-automation preflight.

This check is intentionally configuration-focused. It explains why live
`code` + `ready` work is idle when dogfood engineer automation does not have a
safe coding-workspace binding, without suggesting synthetic PR preparation.
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import stat
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

ENGINEER_ROLE = "engineer"
CODING_WORKSPACE_TOOL = "coding_workspace"


@dataclass(frozen=True)
class EligibleIssue:
    number: int
    title: str
    url: str | None = None


@dataclass(frozen=True)
class Check:
    name: str
    ok: bool
    detail: str


@dataclass
class PreflightReport:
    enable_engineer_automation: bool
    checks: list[Check] = field(default_factory=list)
    eligible_issues: list[EligibleIssue] = field(default_factory=list)
    query_error: str | None = None

    @property
    def blockers(self) -> list[Check]:
        return [check for check in self.checks if not check.ok]

    @property
    def safe_to_enable(self) -> bool:
        return not self.blockers


@dataclass(frozen=True)
class PreflightConfig:
    workflow_file: Path
    roles_env: Path | None
    base_url: str
    owner: str
    repo: str
    enable_engineer_automation: bool
    workspace_root: str
    workspace_command: str
    pr_diff_guard: str
    allow_bookkeeping_only_pr: str
    query_issues: bool = False


def load_env(path: Path | None) -> dict[str, str]:
    if path is None or not path.exists():
        return {}
    env: dict[str, str] = {}
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        try:
            parts = shlex.split(line, posix=True)
        except ValueError:
            continue
        if len(parts) != 1 or "=" not in parts[0]:
            continue
        key, value = parts[0].split("=", 1)
        env[key] = value
    return env


def workflow_declares_tool(workflow_file: Path) -> bool:
    try:
        payload = json.loads(workflow_file.read_text())
    except (OSError, json.JSONDecodeError):
        return False
    for role in payload.get("roles", []):
        if not isinstance(role, dict) or role.get("id") != ENGINEER_ROLE:
            continue
        return any(
            isinstance(tool, dict) and tool.get("id") == CODING_WORKSPACE_TOOL
            for tool in role.get("external_tools", [])
        )
    return False


def roles_env_is_private(path: Path | None) -> bool:
    if path is None or not path.exists() or os.name != "posix":
        return True
    mode = stat.S_IMODE(path.stat().st_mode)
    return mode & 0o077 == 0


def is_git_worktree(path: Path) -> bool:
    try:
        result = subprocess.run(
            ["git", "-C", str(path), "rev-parse", "--is-inside-work-tree"],
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
    except OSError:
        return False
    return result.returncode == 0 and result.stdout.strip() == "true"


def evaluate(config: PreflightConfig, env: dict[str, str] | None = None) -> PreflightReport:
    env = dict(env or load_env(config.roles_env))
    report = PreflightReport(enable_engineer_automation=config.enable_engineer_automation)

    report.checks.append(
        Check(
            "DOGFOOD_ENABLE_ENGINEER_AUTOMATION",
            config.enable_engineer_automation,
            "set to 1 only after every preflight check below is ok"
            if not config.enable_engineer_automation
            else "set to 1",
        )
    )
    report.checks.append(
        Check(
            f"workflow.roles[{ENGINEER_ROLE}].external_tools[{CODING_WORKSPACE_TOOL}]",
            workflow_declares_tool(config.workflow_file),
            f"expected in {config.workflow_file}",
        )
    )

    root_text = config.workspace_root.strip()
    command_text = config.workspace_command.strip()
    root = Path(os.path.expanduser(root_text)) if root_text else None
    report.checks.append(
        Check(
            "TEMPER_CODING_WORKSPACE_ROOT",
            bool(root and root.is_dir()),
            f"directory exists: {root}" if root_text else "not set",
        )
    )
    report.checks.append(
        Check(
            "TEMPER_CODING_WORKSPACE_ROOT git worktree",
            bool(root and root.is_dir() and is_git_worktree(root)),
            "workspace root must be a clean git checkout",
        )
    )
    report.checks.append(
        Check(
            "TEMPER_CODING_WORKSPACE_COMMAND",
            bool(command_text),
            "operator-configured coding provider command is present"
            if command_text
            else "not set",
        )
    )
    report.checks.append(
        Check(
            "DOGFOOD_PR_DIFF_GUARD",
            config.pr_diff_guard.strip() == "1"
            and config.allow_bookkeeping_only_pr.strip() != "1",
            "must be 1 and DOGFOOD_ALLOW_BOOKKEEPING_ONLY_PR must not be 1",
        )
    )
    report.checks.append(
        Check(
            "examples/dogfood/secrets/roles.env mode",
            roles_env_is_private(config.roles_env),
            "must be 0600 when present",
        )
    )
    report.checks.append(
        Check(
            "TEMPER_FORGEJO_TOKEN_ENGINEER",
            bool(env.get("TEMPER_FORGEJO_TOKEN_ENGINEER")),
            "engineer token present in roles.env",
        )
    )
    report.checks.append(
        Check(
            "TEMPER_FORGEJO_TOKEN_OWNER",
            bool(env.get("TEMPER_FORGEJO_TOKEN_OWNER")),
            "owner token present in roles.env for the paired auto-merge worker",
        )
    )

    if config.query_issues:
        token = (
            env.get("TEMPER_FORGEJO_TOKEN_ENGINEER")
            or env.get("DOGFOOD_MECHANICAL_TOKEN")
            or env.get("DOGFOOD_ADMIN_TOKEN")
        )
        if token:
            try:
                report.eligible_issues = list_code_ready_issues(
                    config.base_url, config.owner, config.repo, token
                )
            except Exception as exc:  # network diagnostics only; do not fail disabled preflight
                report.query_error = f"could not query code+ready issues: {exc}"
        else:
            report.query_error = (
                "could not query code+ready issues: no engineer/mechanical/admin token in roles.env"
            )

    return report


def list_code_ready_issues(base_url: str, owner: str, repo: str, token: str) -> list[EligibleIssue]:
    query = urllib.parse.urlencode(
        {
            "state": "open",
            "type": "issues",
            "labels": "code,ready",
            "limit": "50",
        }
    )
    url = (
        base_url.rstrip("/")
        + "/api/v1/repos/"
        + urllib.parse.quote(owner, safe="")
        + "/"
        + urllib.parse.quote(repo, safe="")
        + "/issues?"
        + query
    )
    req = urllib.request.Request(
        url,
        headers={"Authorization": f"token {token}", "Accept": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=20) as resp:
            payload = json.loads(resp.read().decode())
    except urllib.error.HTTPError as exc:
        body = exc.read(200).decode(errors="replace").strip()
        raise RuntimeError(f"GET /issues failed ({exc.code}): {body}") from None
    if not isinstance(payload, list):
        raise RuntimeError("GET /issues returned a non-list response")
    issues: list[EligibleIssue] = []
    for item in payload:
        if not isinstance(item, dict) or item.get("pull_request"):
            continue
        labels = {
            label.get("name")
            for label in item.get("labels", [])
            if isinstance(label, dict)
        }
        if {"code", "ready"}.issubset(labels):
            issues.append(
                EligibleIssue(
                    number=int(item.get("number", 0)),
                    title=str(item.get("title", "(untitled)")),
                    url=item.get("html_url") if isinstance(item.get("html_url"), str) else None,
                )
            )
    return issues


def render_report(report: PreflightReport) -> str:
    lines = ["Dogfood engineer automation preflight"]
    state = "enabled" if report.enable_engineer_automation else "disabled"
    lines.append(f"engineer automation requested: {state}")
    lines.append("")
    lines.append("Checks:")
    for check in report.checks:
        mark = "ok" if check.ok else "blocker"
        lines.append(f"- {mark}: {check.name} — {check.detail}")

    if report.query_error:
        lines.append("")
        lines.append(report.query_error)

    if report.eligible_issues:
        lines.append("")
        if report.safe_to_enable and report.enable_engineer_automation:
            lines.append("Eligible code+ready issues can be serviced by the engineer worker:")
        else:
            lines.append(
                "Eligible code+ready issues are idle because the engineer role lacks a safe coding binding:"
            )
        for issue in report.eligible_issues:
            suffix = f" ({issue.url})" if issue.url else ""
            lines.append(f"- #{issue.number}: {issue.title}{suffix}")

    if report.blockers:
        lines.append("")
        lines.append("Re-enable criteria: fix these specific keys/paths first:")
        for check in report.blockers:
            lines.append(f"- {check.name}")
        lines.append(
            "Then set DOGFOOD_ENABLE_ENGINEER_AUTOMATION=1 for an intentional live issue only."
        )
    else:
        lines.append("")
        lines.append(
            "All re-enable criteria are satisfied; the workspace must still produce a meaningful PR diff for each issue."
        )
    return "\n".join(lines)


def bool_arg(value: str) -> bool:
    return value.strip().lower() in {"1", "true", "yes"}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--workflow-file", required=True)
    parser.add_argument("--roles-env")
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--owner", required=True)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--enable-engineer-automation", default="0")
    parser.add_argument("--workspace-root", default="")
    parser.add_argument("--workspace-command", default="")
    parser.add_argument("--pr-diff-guard", default="1")
    parser.add_argument("--allow-bookkeeping-only-pr", default="0")
    parser.add_argument("--query-issues", action="store_true")
    parser.add_argument("--strict", action="store_true")
    args = parser.parse_args(argv)

    config = PreflightConfig(
        workflow_file=Path(args.workflow_file),
        roles_env=Path(args.roles_env) if args.roles_env else None,
        base_url=args.base_url,
        owner=args.owner,
        repo=args.repo,
        enable_engineer_automation=bool_arg(args.enable_engineer_automation),
        workspace_root=args.workspace_root,
        workspace_command=args.workspace_command,
        pr_diff_guard=args.pr_diff_guard,
        allow_bookkeeping_only_pr=args.allow_bookkeeping_only_pr,
        query_issues=args.query_issues,
    )
    report = evaluate(config)
    print(render_report(report))
    if args.strict and report.enable_engineer_automation and not report.safe_to_enable:
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
