# Basic-delivery observability guide

Use this page while `./run.sh start` is still running. `./run.sh stop` removes
the throwaway Forgejo data, so live Forge-state inspection must happen before
teardown. Logs stay under `logs/` for later inspection.

## Where to look

- `logs/provision.log` — one line for the repo: the seeded site-admin intake URL
  and webhook registration.
- `logs/trigger.log` — webhook trigger readiness (`listening on`), accepted
  Forgejo deliveries, and wake delivery outcomes.
- `logs/architect.log`, `logs/engineer.log` — the two basic-delivery fake role
  workers (`--profile basic`). Each log starts with the configured repo, then
  `temper-testing-worker` tick lines.
- `logs/mechanical.log` — fake mechanical reconciler ticks (stamps intake
  `untriaged`, lands CI-green PRs as the `bot`).
- `logs/runner.log` — real Forgejo Actions runner job execution.
- `logs/forgejo.log` — server-side errors from the throwaway Forgejo.

The fake workers do not call Smith and do not emit role-decision process events.
They log process-level movement such as:

```text
temper-testing-worker: worker 'multi-role:architect' completed tick trigger=wake actions=1 scanned_repositories=1 scanned_repository_paths=acme/service next_poll_ms=120000 idle_no_action_ticks=0
temper-testing-worker: worker 'multi-role:engineer' consumed authenticated wake batch hints=1; ticking immediately
```

## Minimal movement trail

For the default single-repo run, expect:

```text
provision.log: repo=acme/service intake_issue_url=...
trigger.log: webhook accepted repo=acme/service ...
trigger.log: wake_delivery outcome=sent ...
mechanical.log: completed tick trigger=wake actions=1   # marks intake untriaged
architect.log:  completed tick trigger=wake actions=1   # triages to code + ready
engineer.log:   completed tick trigger=wake actions=1   # opens the implementation PR
mechanical.log: completed tick trigger=poll actions=1   # lands the CI-green PR
```

Some wake-triggered ticks may report `actions=0`; that means the worker woke,
read fresh Forge state, and found no eligible item. Mechanical `next_poll_ms` and
`idle_no_action_ticks` show adaptive idle backoff after repeated no-action normal
ticks; wake and audit ticks reset the idle counter. Mechanical logs also include
`mechanical_automation_execution` and `mechanical_automation_summary`; these show
the landing queue, transition, target PR, and whether the item was applied,
gate-blocked, or unchanged.

## Validator

Run while the demo is alive (or after, against the retained logs):

```sh
./run.sh validate-webhooks
```

It confirms that:

- the bot automation credentials are present for landing + CI reads (ADR 0019);
- the repo registered a webhook and produced at least one accepted delivery;
- a site-admin intake issue URL was recorded;
- the trigger sent at least one wake batch;
- every fake worker consumed a wake and completed at least one tick, with at
  least one tick reporting `actions>0`.

If webhook delivery reports persistent `outcome=no_sockets`, a webhook arrived
before workers created their Unix wake sockets, or a worker failed during
startup; inspect the corresponding worker log.

If the PR remains labelled `implementation` and is never `landed`, inspect
`logs/mechanical.log`. The ADR-0019 error:

```text
no web-UI credentials configured for the CI read fallback
```

means the mechanical worker can mutate over REST with the `bot` token but cannot
read Forgejo 7.0.x Actions status. The launcher passes the provisioned `bot`
username/password as `TEMPER_FORGEJO_USERNAME` and `TEMPER_FORGEJO_PASSWORD`;
`validate-webhooks` flags this as a targeted failure. (The setup-only site admin
is never used for automation; the bot is the mechanical worker's identity for both
landing and CI reads.)
