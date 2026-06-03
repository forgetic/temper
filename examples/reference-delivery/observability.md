# Reference-delivery observability guide

Use this page while `./run.sh start` is still running. `./run.sh stop` removes
the throwaway Forgejo data, so live Forge-state validation must happen before
teardown. Logs stay under `logs/` for later inspection.

## Where to look

- `logs/provision.log` — one line per repo, seeded intake URLs, webhook
  registration, and the cross-repo parent URL when enabled.
- `logs/trigger.log` — webhook trigger readiness (`listening on`), accepted
  Forgejo deliveries, and wake delivery outcomes.
- `logs/<role>.log` — one fake role worker per provisioned role. Each log starts
  with the configured repo set, then `temper-testing-worker` tick lines.
- `logs/mechanical.log` — fake mechanical reconciler ticks.
- `logs/runner.log` — real Forgejo Actions runner job execution.
- `logs/forgejo.log` — server-side errors from the throwaway Forgejo.

The fake workers do not call Smith and do not emit role-decision process events.
They log process-level movement such as:

```text
temper-testing-worker: worker 'role:architect' completed tick actions=1
temper-testing-worker: worker 'role:engineer' consumed authenticated wake batch hints=1; ticking immediately
```

## Minimal movement trail

For the default two-repo run, expect:

```text
provision.log: repo=acme/service cross_repo_parent_url=...
trigger.log: webhook accepted repo=acme/service ...
trigger.log: wake_delivery outcome=sent ...
architect.log: completed tick actions=1
engineer.log: completed tick actions=1
reviewer.log: completed tick actions=1
owner.log: completed tick actions=1
mechanical.log: completed tick actions=...
```

Some wake-triggered ticks may report `actions=0`; that means the worker woke,
read fresh Forge state, and found no eligible item.

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
- fake worker logs mention the repo set;
- the live parent issue has the expected child dependencies.

For a stalled fan-out, diagnostics look like:

```text
missing: cross-repo parent acme/service#1 expected 2 child dependencies, found 0
diagnosis: architect blocked the parent but no fan-out side effects were recorded
```

If webhook delivery reports persistent `outcome=no_sockets`, a webhook arrived
before workers created their Unix wake sockets or a worker failed during startup;
inspect the corresponding worker log.
