# Reference-delivery observability guide

Use this page while `./run.sh start` is still running. `./run.sh stop` removes the
throwaway Forgejo data, so live Forge-state validation must happen before
teardown. Logs stay under `logs/` for later inspection.

The whole topology is **one process** (`temper run`), so the daemon, the
in-process worker, and the coding agent all write to a **single log**:
`logs/run.log`. Daemon lines are prefixed `engine:`; worker lines
`worker:`.

## Where to look

- `logs/provision.log` — one line per repo: seeded intake URLs, webhook
  registration, and the cross-repo parent URL when enabled.
- `logs/run.log` — the unified daemon + worker + agent. Serving readiness,
  accepted webhook deliveries + wake scans, daemon job assignments and results
  (with `repo=` for per-repo attribution), worker register/assign/result lines,
  cross-repo child materialisation, and mechanical automation/landing.
- `logs/ci-seed.log` — the one-time engineer clone + push that installs the
  bundled pass-through CI before `temper run` starts.
- `logs/runner.log` — real Forgejo Actions runner job execution.
- `logs/forgejo.log` — server-side errors from the throwaway Forgejo.

## Minimal movement trail

For the default two-repo cross-repo run, expect (interleaved) in `logs/run.log`:

```text
engine:  serving on 127.0.0.1:38200
worker: registered worker_id=reference-delivery-1 capabilities=6
engine: webhook accepted repo=acme/service kind=Issue item=<n>
engine: webhook wake scan repo=acme/service enqueued=1
engine: assigned job_id=... role=architect repo=acme/service worker=reference-delivery-1
worker: result sent job_id=... status=success     # fans the parent into per-repo children
engine: assigned job_id=... role=engineer repo=acme/service ...
engine: assigned job_id=... role=engineer repo=acme/service-canary ...
worker: result sent job_id=... status=success     # opens each implementation PR
engine: assigned job_id=... role=reviewer repo=... worker=reference-delivery-1
worker: result sent job_id=... status=success     # reviewer approves
engine: ... mechanical landing ...                # bot merges each CI-green, approved PR
```

The seed-last webhook-wake proof is the `webhook accepted` → `webhook wake scan …
enqueued=1` pair: intake is filed only after `temper run` is ready, so its creation
webhook (not the slow poll backstop) drives the first scan. Mechanical logs also
include `mechanical_automation_summary` / `mechanical_reconciliation_summary` lines
(high volume), and after a merge rejection a merge-conflict route; these show the
landing queue, transition, target PR, and whether the item was applied,
gate-blocked, unchanged, or routed to `merge-conflict`.

## Validators

Run while the demo is alive:

```sh
./run.sh validate-webhooks
./run.sh validate-multi-repo
```

`validate-multi-repo` checks that:

- every configured repo was provisioned;
- only the source repo received the parent intake in cross-repo mode;
- every repo registered a webhook and produced at least one accepted delivery;
- the run assigned at least one daemon job and one worker job per repo (matched on
  `repo=` in `logs/run.log`);
- the live parent issue has the expected child dependencies.

For a stalled fan-out, diagnostics look like:

```text
missing: cross-repo parent acme/service#1 expected 2 child dependencies, found 0
diagnosis: architect blocked the parent but no fan-out side effects were recorded
```

If PRs remain labelled `landing`, inspect `logs/run.log`. The ADR-0019 error:

```text
no web-UI credentials configured for the CI read fallback
```

means the run can mutate over REST with the `bot` token but cannot read Forgejo
7.0.x Actions status. The launcher passes the provisioned `bot` username/password
as `FORGEJO_USERNAME` / `FORGEJO_PASSWORD`; `validate-webhooks` and
`validate-multi-repo` flag this as a targeted failure. (The setup-only site admin
is never used here; the bot is the mechanical identity for both landing and CI
reads.)
