# Reference-delivery observability guide

Use this page while `./run.sh start` is still running. `./run.sh stop` removes the
throwaway Forgejo data. Logs stay under `logs/` for later inspection.

The default topology is a deterministic cross-repo worker fleet: separate
`temper-testing-worker` processes for mechanical, architect, engineer, and
reviewer roles. These workers are normally quiet on stdout/stderr unless they hit
an error; the durable movement proof is the Forgejo UI, the provisioning summary,
and the real runner log. The optional `./run.sh single-repo` path uses one
`temper serve standalone` process plus local jig and writes `logs/run.log` /
`logs/jig.log`.

## Where to look

- `logs/provision.log` — repository provisioning summary, source parent issue
  number, repo set, and worker fleet summary.
- `logs/worker-*.log` — deterministic worker stdout/stderr, normally quiet
  unless a worker reports an error.
- `logs/runner.log` — real Forgejo Actions runner job execution.
- `logs/forgejo.log` — server-side errors from the throwaway Forgejo.
- `logs/run.log`, `logs/jig.log`, and `logs/repo-populate.log` — optional
  `single-repo` standalone path only.

## Minimal movement trail

For the default cross-repo run, expect this shape across `logs/provision.log` and
`logs/runner.log`, with issue and PR state visible in the Forgejo UI:

```text
source_repo=acme/service target_repos=acme/service acme/service-canary expected_children=2
repo=acme/service cross_repo_parent_issue_number=1 expected_children=2 ...
repo_set="acme/service acme/service-canary" workers=mechanical,architect,engineer,reviewer ...
[ci/build] commit ... [ci-pass]
[ci/build] 🏁  Job succeeded
```

The key shape is one source parent, no duplicate canary parent, exactly two
child dependencies distributed one per repository, reviewer-approved green PRs,
and parent closure only after both child issues land.

## Forgejo checks

Before stopping the default run, open Forgejo at the URL printed by `run.sh` and
confirm that both repositories are readable, the source repo has the single
parent intake, the canary repo has no duplicate parent intake, the parent records
one child issue per repository, each child has a merged implementation PR with an
approving review and successful Actions run, and the parent closes after both
child issues close.

For `./run.sh single-repo`, use `logs/run.log` to inspect the standalone engine
and worker events after the run.
