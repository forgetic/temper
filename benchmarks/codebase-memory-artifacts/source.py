"""Pinned source inspection and independent, ownership-safe checkouts."""
import json
import os
import subprocess
from pathlib import Path

from contract import Refusal, digest, identity


def git_environment():
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    environment.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null",
                        "GIT_CONFIG_SYSTEM": "/dev/null", "GIT_TERMINAL_PROMPT": "0"})
    return environment


def git(root, *args):
    result = subprocess.run(["git", "-c", f"safe.directory={Path(root).resolve()}", "-C", str(root), *args],
                            env=git_environment(), check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    return result.stdout


def ignore_identity(repository, commit):
    names = git(repository, "ls-tree", "-r", "--name-only", "-z", commit).split(b"\0")
    policies = []
    for name in names:
        if name and Path(os.fsdecode(name)).name in (".gitignore", ".cbmignore", ".codebase-memory.json"):
            path = os.fsdecode(name)
            policies.append([path, digest(git(repository, "show", f"{commit}:{path}"))])
    # Clones use Git's default info/exclude template; explicit empty global policy.
    return identity({"tracked_policies": policies, "global_git_excludes": "disabled",
                     "clone_info_exclude": "empty", "provider_user_extensions": "absent"})


def inspect_source(repository, commit):
    resolved = git(repository, "rev-parse", "--verify", commit + "^{commit}").decode().strip()
    return {"commit": resolved, "ignore_sha256": ignore_identity(repository, resolved)}


def validate_sources(config):
    source = config["source"]
    for rev in ("p0", "p1"):
        observed = inspect_source(source["repository"], source[rev])
        if observed != {"commit": source[rev], "ignore_sha256": source["ignore_sha256"][rev]}:
            raise Refusal("source-or-ignore-identity-mismatch")
        for symbol in config["queries"][rev]["symbols"]:
            lines = git(source["repository"], "show", f'{source[rev]}:{symbol["file"]}').splitlines(keepends=True)
            exact = b"".join(lines[symbol["start_line"] - 1:symbol["end_line"]])
            if digest(exact) != symbol["source_sha256"]:
                raise Refusal("frozen-query-source-mismatch")
    git(source["repository"], "merge-base", "--is-ancestor", source["p0"], source["p1"])
    changes = git(source["repository"], "diff", "--name-status", "--find-renames", source["p0"], source["p1"])
    kinds = {line.split(b"\t")[0][:1].decode() for line in changes.splitlines()}
    if not {"A", "M", "D", "R"}.issubset(kinds):
        raise Refusal("missing-controlled-mutation-kind")
    return {"mutation_kinds": sorted(kinds), "ancestry_confirmed": True}


def own_tree(root, uid, gid):
    """Never follow symlinks; callers first clone with --no-hardlinks."""
    os.chown(root, uid, gid, follow_symlinks=False)
    for directory, dirs, files in os.walk(root, followlinks=False):
        for name in dirs + files:
            os.chown(Path(directory) / name, uid, gid, follow_symlinks=False)


def checkout(repository, commit, destination, uid, gid):
    subprocess.run(["git", "-c", f"safe.directory={Path(repository).resolve()}", "clone",
                    "--no-hardlinks", "--no-checkout", "--template=", "--quiet",
                    "-c", "core.autocrlf=false", str(repository), str(destination)],
                   env=git_environment(), check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    git(destination, "checkout", "--quiet", "--detach", commit)
    git(destination, "config", "core.excludesFile", "/dev/null")
    (destination / ".git/info").mkdir(exist_ok=True)
    (destination / ".git/info/exclude").write_bytes(b"")
    if git(destination, "config", "--get-all", "core.excludesFile").strip() != b"/dev/null":
        raise Refusal("clone-ignore-policy-unconfirmed")
    if (destination / ".git/info/exclude").read_bytes():
        raise Refusal("clone-ignore-policy-unconfirmed")
    if (destination / ".codebase-memory").exists():
        raise Refusal("source-already-contains-artifact")
    own_tree(destination, uid, gid)


def confirm_checkout(root, commit):
    if git(root, "rev-parse", "HEAD").decode().strip() != commit:
        raise Refusal("checkout-commit-mismatch")
    # Only intentional provider-generated files may be untracked.
    status = git(root, "status", "--porcelain", "--untracked-files=all").decode().splitlines()
    if any(not line.startswith("?? .codebase-memory/") for line in status):
        raise Refusal("checkout-source-dirty")


def write_json(path, value):
    Path(path).write_text(json.dumps(value, sort_keys=True, indent=2) + "\n")
