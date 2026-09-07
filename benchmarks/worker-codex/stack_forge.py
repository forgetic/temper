"""Forgejo issue creation and observed delivery evidence for one benchmark."""

import json
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen
from urllib.parse import quote

from stack_support import private_json


class ForgeError(RuntimeError):
    def __init__(self, message, status):
        super().__init__(message)
        self.status = status


class Forge:
    def __init__(self, base_url, token, repository="acme/repo"):
        self.base_url, self.token, self.repository = base_url, token, repository
        self.deadline = None

    def request(self, path, data=None, method=None):
        timeout = 15 if self.deadline is None else min(15, self.deadline - time.monotonic())
        if timeout <= 0:
            raise TimeoutError("benchmark delivery deadline exhausted before Forgejo request")
        request = Request(self.base_url + "/api/v1/" + path,
                          data=json.dumps(data).encode() if data is not None else None,
                          method=method or ("POST" if data is not None else "GET"),
                          headers={"Authorization": f"token {self.token}",
                                   "Content-Type": "application/json"})
        try:
            with urlopen(request, timeout=timeout) as response:
                body = response.read()
        except HTTPError as error:
            raise ForgeError(f"Forgejo {request.method} {path} returned HTTP {error.code}", error.code) from None
        return json.loads(body) if body else None

    def repo(self, path, **kwargs):
        return self.request(f"repos/{self.repository}/{path}", **kwargs)

    def await_branch(self, branch, sha, timeout=30):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                response = self.repo("branches/" + quote(branch, safe=""))
                commit = response.get("commit", {})
                if commit.get("id", commit.get("sha")) == sha:
                    return
            except ForgeError as error:
                if error.status != 404:
                    raise
            time.sleep(0.2)
        raise TimeoutError(f"Forgejo did not expose branch {branch} at the seeded commit")

    def create_code_issue(self, title, task):
        labels = self.repo("labels?limit=100")
        ids = {label["name"]: label["id"] for label in labels}
        if not {"code", "ready"}.issubset(ids):
            raise RuntimeError("basic delivery did not provision code and ready labels")
        return self.repo("issues", data={"title": title, "body": task,
                                         "labels": [ids["code"], ids["ready"]]})

    def observe_delivery(self, issue_number):
        pulls = self.repo("pulls?state=all&limit=100")
        implementation = [pr for pr in pulls
                          if any(label["name"] == "implementation" for label in pr["labels"])]
        if len(implementation) > 1:
            raise RuntimeError("benchmark produced more than one implementation PR")
        issue = self.repo(f"issues/{issue_number}")
        if not implementation:
            return issue, None, False
        pr = self.repo(f"pulls/{implementation[0]['number']}")
        if f"pr-for-code-{issue_number}" not in pr.get("body", ""):
            raise RuntimeError("implementation PR lacks source issue correlation metadata")
        done = pr.get("merged", False) and issue["state"] == "closed"
        return issue, pr, done

    def ci_evidence(self, head_sha):
        runs = self.repo("actions/runs?limit=100")
        entries = runs.get("workflow_runs", runs.get("runs", [])) if isinstance(runs, dict) else runs
        return [run for run in entries if run.get("commit_sha", run.get("head_sha")) == head_sha]


def await_delivery(stack, issue_number, deadline):
    first_pr_ns = None
    while time.monotonic() < deadline:
        if stack.process.poll() is not None:
            raise RuntimeError("standalone Temper exited; inspect temper.log")
        if stack.helper.poll() is not None:
            raise RuntimeError("isolated Forgejo fixture exited; inspect fixture.log")
        issue, pr, done = stack.forge.observe_delivery(issue_number)
        if pr and first_pr_ns is None:
            first_pr_ns = time.monotonic_ns()
        if done:
            merged_by = (pr.get("merged_by") or {}).get("login")
            author = (pr.get("user") or {}).get("login")
            if not merged_by or not author or merged_by == author:
                raise RuntimeError("implementation author merged the PR instead of automation")
            if "landing" in {label["name"] for label in pr["labels"]}:
                time.sleep(0.2)
                continue
            ci = stack.forge.ci_evidence(pr["head"]["sha"])
            if not ci or not ci_passed(ci):
                time.sleep(0.2)
                continue
            private_json(stack.root / "forge-evidence.json", {"issue": issue, "pull_request": pr, "ci_runs": ci})
            return issue, pr, ci, first_pr_ns, time.monotonic_ns()
        labels = {label["name"] for label in issue.get("labels", [])}
        if "needs-human" in labels:
            private_json(stack.root / "failed-issue.json", issue)
            raise RuntimeError("benchmark issue parked needs-human; inspect retained evidence")
        time.sleep(0.5)
    raise TimeoutError(f"issue #{issue_number} exceeded the delivery deadline")


def ci_passed(runs):
    """Require the latest exact-head run of every observed workflow to pass."""
    latest = {}
    for run in runs:
        key = run.get("workflow_id", run.get("name", "ci"))
        if key not in latest or run["id"] > latest[key]["id"]:
            latest[key] = run
    return bool(latest) and all(
        run.get("conclusion", run.get("status")) == "success"
        for run in latest.values()
    )
