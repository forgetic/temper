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
trigger: webhook listener up on 127.0.0.1:38200/forgejo/webhook (issue, PR, CI events)
worker:  capacity: architect=1 engineer=1 reviewer=1 (per-role, shared across all repos)
engine:  ready -- watching acme/service, acme/service-canary, idle
trigger: [acme/service#1] wake | artifact=intake queue=raw_intake ... event="wake.received"
engine:  [acme/service#1] mark_untriaged applied | +untriaged
agent:   [acme/service#1] architect/triage start ... event="agent.started"
agent:   [acme/service#1] architect/triage done ... event="agent.finished" # fans the parent into per-repo children
agent:   [acme/service#2] engineer/coding start ...
agent:   [acme/service-canary#1] engineer/coding start ...
agent:   [...] engineer/coding done ...              # opens each implementation PR
agent:   [...] reviewer/review start ...
agent:   [...] reviewer/review done ...              # reviewer approves
engine:  [...] merged -> main ... event="pr.merged" # bot merges each CI-green, approved PR
engine:  [...] resolved -- implemented by PR#...     # source issue closed by close_parent_issues
```

The seed-last webhook-wake proof is the `event="wake.received"` line followed by
`mark_untriaged applied`: intake is filed only after `temper run` is ready, so its
creation webhook (not the slow poll backstop) drives the first transition.
Mechanical logs also include `mechanical_automation_summary` /
`mechanical_reconciliation_summary` lines
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
