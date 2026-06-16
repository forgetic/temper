# Basic-delivery observability guide

Use this page while `./run.sh start` is still running. `./run.sh stop` removes the
throwaway Forgejo data, so live Forge-state inspection must happen before
teardown. Logs stay under `logs/` for later inspection.

The whole topology is **one process** (`temper run`), so the daemon, the
in-process worker, and the coding agent all write to a **single log**:
`logs/run.log`. Daemon lines are prefixed `engine:`; worker lines
`worker:`.

## Where to look

- `logs/provision.log` — one line for the repo: the seeded site-admin intake URL
  and webhook registration.
- `logs/run.log` — the unified daemon + worker + agent. Serving readiness,
  accepted webhook deliveries + wake scans, daemon job assignments and results,
  worker register/assign/result lines, and mechanical automation/landing.
- `logs/ci-seed.log` — the one-time engineer clone + push that installs the
  bundled pass-through CI before `temper run` starts.
- `logs/runner.log` — real Forgejo Actions runner job execution.
- `logs/forgejo.log` — server-side errors from the throwaway Forgejo.

## Minimal movement trail

For the default single-repo run, expect (interleaved) in `logs/run.log`:

```text
engine: serving on 127.0.0.1:38100
worker: registered worker_id=basic-delivery-1 capabilities=2
engine: webhook accepted repo=acme/service kind=Issue item=<n>
engine: webhook wake scan repo=acme/service enqueued=1
engine: assigned job_id=... role=architect repo=acme/service worker=basic-delivery-1
worker: assigned job_id=... role=architect repo=acme/service
worker: result sent job_id=... status=success         # ready_code + rewritten body
engine: result received job_id=... status=success disposition=...
engine: assigned job_id=... role=engineer repo=acme/service worker=basic-delivery-1
worker: result sent job_id=... status=success         # opens the implementation PR
engine: ... mechanical landing ...                    # lands the CI-green PR as bot
```

The seed-last webhook-wake proof is the `webhook accepted` → `webhook wake scan …
enqueued=1` pair: the intake issue is filed only after `temper run` is ready, so
its creation webhook (not the slow poll backstop) drives the first scan. The
mechanical backstop stamps the raw intake `untriaged` and, once CI is green,
merges the PR as `bot`. Mechanical logs also include
`mechanical_automation_summary` / `mechanical_reconciliation_summary` lines (high
volume); these show the landing queue, transition, target PR, and whether the item
was applied, gate-blocked, or unchanged.

## Validator

Run while the demo is alive (or after, against the retained logs):

```sh
./run.sh validate-webhooks
```

It confirms that:

- the bot automation credentials are present for landing + CI reads (ADR 0019);
- the repo registered a webhook and produced at least one accepted delivery;
- a site-admin intake issue URL was recorded;
- `temper run` reached serving readiness and ran at least one webhook wake scan
  that enqueued work;
- the daemon assigned at least one job and received at least one result;
- the in-process worker registered, accepted an assignment, and sent a result.

If the PR remains labelled `implementation` and is never `landed`, inspect
`logs/run.log`. The ADR-0019 error:

```text
no web-UI credentials configured for the CI read fallback
```

means the run can mutate over REST with the `bot` token but cannot read Forgejo
7.0.x Actions status. The launcher passes the provisioned `bot` username/password
as `FORGEJO_USERNAME` / `FORGEJO_PASSWORD`; `validate-webhooks` flags this as a
targeted failure. (The setup-only site admin is never used for automation; the bot
is the mechanical identity for both landing and CI reads.)
