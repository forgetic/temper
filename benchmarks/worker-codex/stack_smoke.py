"""Opt-in real-stack provisioning and host CI smoke; never starts a model job."""

import argparse
from pathlib import Path
import tempfile
import time

from stack import start
from stack_forge import ci_passed
from stack_support import git, private_json


def smoke(temper_bin, fixture_bin, auth_file, root):
    seed = root / "input"
    (seed / ".forgejo/workflows").mkdir(parents=True)
    (seed / "README.md").write_text("CI provisioning smoke only.\n")
    (seed / ".forgejo/workflows/ci.yml").write_text(
        "name: ci\non: [pull_request]\njobs:\n  check:\n    runs-on: host\n"
        "    steps:\n      - run: echo CI-smoke-passed\n"
    )
    with start(seed=seed, root=root / "stack", temper_bin=temper_bin,
               fixture_bin=fixture_bin, auth_file=auth_file) as stack:
        server_dir = Path(stack.bootstrap["server_data_dir"])
        runner_dir = Path(stack.bootstrap["runner_work_dir"])
        git(["checkout", "-b", "ci-smoke"], cwd=stack.seed_checkout)
        (stack.seed_checkout / "README.md").write_text("CI smoke change.\n")
        git(["add", "README.md"], cwd=stack.seed_checkout)
        git(["commit", "-m", "CI smoke"], cwd=stack.seed_checkout)
        sha = git(["rev-parse", "HEAD"], cwd=stack.seed_checkout)
        git(["push", "origin", "HEAD"], cwd=stack.seed_checkout,
            token=stack.bootstrap["admin_token"], log=stack.root / "ci-push.log")
        stack.forge.await_branch("ci-smoke", sha)
        stack.forge.repo("pulls", data={"base": "main", "head": "ci-smoke",
                                        "title": "CI smoke without coding issue"})
        deadline = time.monotonic() + 120
        runs = []
        while time.monotonic() < deadline:
            runs = stack.forge.ci_evidence(sha)
            if ci_passed(runs):
                break
            time.sleep(0.5)
        if not ci_passed(runs):
            raise RuntimeError("exact-head host CI smoke did not pass")
        private_json(root / "ci-smoke.json", {"passed": True, "head_sha": sha,
                     "run_ids": [run["id"] for run in runs], "model_jobs": 0})
    if server_dir.exists() or runner_dir.exists() or stack.helper.returncode != 0:
        raise RuntimeError("fixture teardown failed")
    if "agent.started" in (stack.root / "temper.log").read_text():
        raise RuntimeError("provisioning smoke unexpectedly started a model job")
    if (stack.bundle / "credentials.toml").exists():
        raise RuntimeError("ephemeral credentials survived teardown")
    return root


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--temper-bin", type=Path, required=True)
    parser.add_argument("--fixture-bin", type=Path, required=True)
    parser.add_argument("--auth-file", type=Path, required=True)
    args = parser.parse_args()
    smoke_root = Path(tempfile.mkdtemp(prefix="temper-stack-ci-smoke-"))
    print(f"smoke_root={smoke_root}", flush=True)
    smoke(args.temper_bin, args.fixture_bin, args.auth_file, smoke_root)
    print(f"passed: host CI, no model jobs, teardown; artifacts={smoke_root}")
