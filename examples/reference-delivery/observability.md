# Reference-delivery observability guide

Use this page while `./run.sh start` is still running. `./run.sh stop` removes the
throwaway Forgejo data. Logs stay under `logs/` for later inspection.

The default topology is a deterministic cross-repo worker fleet: separate
`temper-testing-worker` processes for mechanical, architect, engineer, and
reviewer roles. These workers are normally quiet on stdout/stderr unless they hit
an error; the durable movement proof is the live Forge-state validator plus the
real runner log. The optional `./run.sh single-repo` path uses one `temper serve
standalone` process plus local jig and writes `logs/run.log` / `logs/jig.log`.

## Where to look

- `logs/provision.log` — repository provisioning summary, source parent issue
  number, repo set, and worker fleet summary.
- `logs/worker-*.log` — deterministic worker stdout/stderr, normally quiet
  unless a worker reports an error.
- `logs/validate-multi-repo.log` — live Forge-state validation once the default
  run converges.
- `logs/runner.log` — real Forgejo Actions runner job execution.
- `logs/forgejo.log` — server-side errors from the throwaway Forgejo.
- `logs/run.log`, `logs/jig.log`, and `logs/repo-populate.log` — optional
  `single-repo` standalone path only.

## Minimal movement trail

For the default cross-repo run, expect this shape across `logs/provision.log`,
`logs/runner.log`, and `logs/validate-multi-repo.log`:

```text
source_repo=acme/service target_repos=acme/service acme/service-canary expected_children=2
repo=acme/service cross_repo_parent_issue_number=1 expected_children=2 ...
repo_set="acme/service acme/service-canary" workers=mechanical,architect,engineer,reviewer ...
[ci/build] commit ... [ci-pass]
[ci/build] 🏁  Job succeeded
ok: repository acme/service has exactly one child dependency from the parent
ok: repository acme/service-canary has exactly one child dependency from the parent
ok: child landed count 2/2 (closed issues count as landed dependency targets)
ok: parent acme/service#1 is closed after all children landed
```

The key shape is one source parent, no duplicate canary parent, exactly two
child dependencies distributed one per repository, reviewer-approved green PRs,
and parent closure only after both child issues land.

## Validator

Run while the default demo is alive (the validator uses the run-local bot token):

```sh
./run.sh validate
```

For the default run, the validator checks that both repositories are readable,
the source repo has the single parent intake, the canary repo has no duplicate
parent intake, the parent records exactly two child dependencies, each child
carries parent/correlation metadata and is closed, and the parent closes no
earlier than the latest child landing. For `./run.sh single-repo`, `validate`
falls back to the retained standalone log checks.
