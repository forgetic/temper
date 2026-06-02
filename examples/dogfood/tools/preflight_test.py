#!/usr/bin/env python3
from __future__ import annotations

import os
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

import preflight


REPO_ROOT = Path(__file__).resolve().parents[3]
WORKFLOW = REPO_ROOT / "crates/harness-workflow/fixtures/reference-delivery.json"


class PreflightTests(unittest.TestCase):
    def config(self, **overrides: object) -> preflight.PreflightConfig:
        values = {
            "workflow_file": WORKFLOW,
            "roles_env": None,
            "base_url": "https://git.example.invalid",
            "owner": "ai",
            "repo": "harness",
            "enable_engineer_automation": False,
            "workspace_root": "",
            "workspace_command": "",
            "pr_diff_guard": "1",
            "allow_bookkeeping_only_pr": "0",
            "query_issues": False,
        }
        values.update(overrides)
        return preflight.PreflightConfig(**values)  # type: ignore[arg-type]

    def test_disabled_ready_issue_explains_idle_keys_without_synthetic_prep(self) -> None:
        report = preflight.evaluate(
            self.config(),
            env={"HARNESS_FORGEJO_TOKEN_ENGINEER": "tok", "HARNESS_FORGEJO_TOKEN_OWNER": "tok"},
        )
        report.eligible_issues = [preflight.EligibleIssue(7, "Implement a real dogfood fix")]

        rendered = preflight.render_report(report)

        self.assertIn("Eligible code+ready issues are idle", rendered)
        self.assertIn("DOGFOOD_ENABLE_ENGINEER_AUTOMATION", rendered)
        self.assertIn("HARNESS_CODING_WORKSPACE_ROOT", rendered)
        self.assertIn("HARNESS_CODING_WORKSPACE_COMMAND", rendered)
        self.assertNotIn("synthetic", rendered.lower())
        self.assertNotIn("--allow-synthetic-pr-prep", rendered)

    def test_enable_requires_declaration_binding_guard_and_credentials(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            workflow = Path(tmp) / "workflow.json"
            workflow.write_text('{"name":"x","roles":[{"id":"engineer","queues":[]}]}')
            report = preflight.evaluate(
                self.config(
                    workflow_file=workflow,
                    enable_engineer_automation=True,
                    pr_diff_guard="0",
                ),
                env={},
            )

        blockers = {check.name for check in report.blockers}
        self.assertIn("workflow.roles[engineer].external_tools[coding_workspace]", blockers)
        self.assertIn("HARNESS_CODING_WORKSPACE_ROOT", blockers)
        self.assertIn("HARNESS_CODING_WORKSPACE_COMMAND", blockers)
        self.assertIn("DOGFOOD_PR_DIFF_GUARD", blockers)
        self.assertIn("HARNESS_FORGEJO_TOKEN_ENGINEER", blockers)
        self.assertIn("HARNESS_FORGEJO_TOKEN_OWNER", blockers)

    def test_enable_passes_with_private_roles_env_and_git_workspace(self) -> None:
        if shutil.which("git") is None:
            self.skipTest("git is required for the workspace preflight")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "checkout"
            root.mkdir()
            subprocess.run(["git", "-C", str(root), "init", "-q"], check=True)
            roles = Path(tmp) / "roles.env"
            roles.write_text(
                "HARNESS_FORGEJO_TOKEN_ENGINEER='engineer-token'\n"
                "HARNESS_FORGEJO_TOKEN_OWNER='owner-token'\n"
            )
            os.chmod(roles, stat.S_IRUSR | stat.S_IWUSR)

            report = preflight.evaluate(
                self.config(
                    roles_env=roles,
                    enable_engineer_automation=True,
                    workspace_root=str(root),
                    workspace_command="coder --context $HARNESS_CODING_WORKSPACE_CONTEXT",
                )
            )

        self.assertTrue(report.safe_to_enable, preflight.render_report(report))

    def test_roles_env_loader_handles_shell_quotes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            roles = Path(tmp) / "roles.env"
            roles.write_text(r"HARNESS_FORGEJO_TOKEN_ENGINEER='tok-with-'\''-quote'" + "\n")

            env = preflight.load_env(roles)

        self.assertEqual(env["HARNESS_FORGEJO_TOKEN_ENGINEER"], "tok-with-'-quote")


if __name__ == "__main__":
    unittest.main()
