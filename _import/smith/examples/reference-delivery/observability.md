# Reference-delivery observability guide

Use this page while `./run.sh start` is still running. `./run.sh stop` removes
the throwaway Forgejo data and runtime workspaces, so Forge-state validation must
happen before teardown. Logs stay under `logs/` for later inspection.

The reference-delivery launcher now has two workflow processes to follow:
`logs/daemon.log` for queue/webhook/apply/landing authority and
`logs/worker.log` for Smith coding-executor jobs. The agent run itself happens
inside the worker; its stdout and stderr are captured in the worker log.

## Daemon log: `logs/daemon.log`

Start with readiness. A healthy daemon prints:

```text
temper-daemon: serving on ...
```

The seed-last webhook path should then produce this chain for the intake issue or
cross-repo parent:

```text
temper-daemon: webhook accepted repo=... kind=... item=...
temper-daemon: webhook wake scan repo=... enqueued=N
temper-daemon: assigned job_id=... role=... repo=... worker=...
temper-daemon: result received job_id=... worker=... status=... disposition=...
```

`enqueued=N` should be positive for at least one wake scan in a moving run. The
first assigned jobs are usually the architect triage/breakdown jobs after the
mechanical intake mark; later assignments cover engineer and reviewer work.

Polling is the liveness backstop, so an otherwise quiet run may also show:

```text
temper-daemon: poll backstop enqueued=N
```

That line means the daemon recovered or rechecked work through its cadence rather
than through a webhook. It is useful for correctness, but the demonstrated fast
path is the webhook wake scan.

The daemon also emits skip guards that prove churn protection is active. These
are expected when a scan sees terminal or already-served artifacts:

```text
temper-daemon: skipped role work for terminal artifact
temper-daemon: skipped role work for existing implementation pull request
```

Apply-window and recently-applied skip lines serve the same purpose: they show
the daemon declined to enqueue duplicate or unsafe work while Forge state was
still settling:

```text
temper-daemon: skipped enqueue for job in apply window job_id=...
temper-daemon: skipped enqueue for recently applied job job_id=...
```

Mechanical landing and reconciliation also live in the daemon. For landing, look
for `mechanical_automation_execution` JSON events that include the target PR,
for example a payload containing:

```text
"artifact_number"
```

That is the daemon-hosted bot automation path reading CI/review state and running
the workflow's `land_pr` transition.

## Worker log: `logs/worker.log`

The worker has a much smaller evidence chain. A healthy worker registers once:

```text
smith-worker: registered worker_id=... capabilities=N
```

For each assigned job, it prints:

```text
smith-worker: assigned job_id=... role=... repo=...
smith-worker: result sent job_id=... status=...
```

Join these lines to the daemon by `job_id`. Join them to Forgejo by `role`,
`repo`, and the issue/PR that moved immediately after the result was received.
A `status=success` result means the worker produced a protocol-valid result; the
subsequent daemon `disposition=...` tells you how the daemon applied it.

## Minimal movement trail

For one happy-path item, the two logs should show this order:

```text
temper-daemon: serving on ...
smith-worker: registered worker_id=... capabilities=N
temper-daemon: webhook accepted repo=... kind=... item=...
temper-daemon: webhook wake scan repo=... enqueued=N
temper-daemon: assigned job_id=... role=architect repo=... worker=...
smith-worker: assigned job_id=... role=architect repo=...
smith-worker: result sent job_id=... status=success
temper-daemon: result received job_id=... worker=... status=success disposition=...
temper-daemon: assigned job_id=... role=engineer repo=... worker=...
smith-worker: assigned job_id=... role=engineer repo=...
smith-worker: result sent job_id=... status=success
temper-daemon: result received job_id=... worker=... status=success disposition=...
temper-daemon: assigned job_id=... role=reviewer repo=... worker=...
smith-worker: assigned job_id=... role=reviewer repo=...
smith-worker: result sent job_id=... status=success
temper-daemon: result received job_id=... worker=... status=success disposition=...
```

In Forgejo, that corresponds to: intake marked `untriaged`, architect rewrite to
`code` + `ready`, implementation PR created with `implementation` +
`needs-reviewer`, reviewer approval and `landing`, then bot merge. The bot merge
also appears in `logs/daemon.log` as a `mechanical_automation_execution` JSON
line containing `"artifact_number"`.

## Cross-repo evidence

For cross-repo mode, first find the architect breakdown result:

```text
temper-daemon: result received job_id=... worker=... status=success disposition=...
```

The corresponding job is the architect assignment for the parent repo. After that
result is applied, open the Forgejo UI and inspect:

- the parent issue in the first configured repo;
- one child code issue in each configured target repo;
- each child body's workflow metadata block, which includes the repo-qualified
  parent backref and child correlation metadata;
- the parent's dependency refs, which point to the child issues.

`./run.sh validate-multi-repo` is the scripted version of that check. It requires
per-repo provisioning and assignment evidence, then runs Temper's
`temper-validate-reference-delivery` against live Forge state to verify the
child-per-repo fan-out, parent backrefs, child correlation metadata, and parent
dependency refs. Run it before teardown.

## Forge-side evidence

The logs prove process movement; Forgejo proves authority and native state.
Useful UI checks:

- role attribution: implementation PRs are authored by the engineer identity,
  reviews by the reviewer identity, and merges by `bot`;
- PR creation labels: implementation PRs appear with `implementation` and
  `needs-reviewer` immediately because those labels come from the workflow's
  artifact definition;
- native review state: reviewer `approve` produces an actual approval review,
  while `changes` would produce a native changes-requested review carrying the
  authored body;
- landing: approved, CI-green PRs receive `landing`, then the daemon's mechanical
  backstop merges them as `bot` and Forgejo closes the source issue through the
  merge trailer.

## Validators

`./run.sh validate-webhooks` checks the generic two-process evidence chain:
webhook registration in `logs/provision.log`, daemon readiness, accepted webhook,
wake scan, assigned job, received result, worker registration, worker assignment,
worker result send, and daemon mechanical bot credential health.

`./run.sh validate-multi-repo` includes those checks plus per-repo evidence. When
cross-repo intake is enabled, it also validates live Forge state for the parent
and children. It is the validator to run when you are checking fan-out.

All event payloads are bounded and omit secrets. Role tokens and provider
credentials should not appear in these logs.
