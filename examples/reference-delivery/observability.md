# Reference-delivery observability guide

Use this page while `./run.sh start` is still running. `./run.sh stop` removes the
throwaway Forgejo data. Logs stay under `logs/` for later inspection.

The whole Temper topology is **one process** (`temper serve standalone`), so the
engine, in-process worker, and coding agent all write to `logs/run.log`. The
local jig LLM is a tiny helper process with its own `logs/jig.log`.

## Where to look

- `logs/provision.log` — init/apply summary, webhook registration, initial repo
  commit, provider fixture URL, and the seeded intake issue URL.
- `logs/run.log` — standalone readiness, accepted webhook deliveries, queue
  transitions, architect/engineer/reviewer agent events, PR gate evaluations,
  merge events, and source issue resolution.
- `logs/jig.log` — the deterministic local LLM endpoint URL.
- `logs/repo-populate.log` — the one-time admin push that installs the bundled CI
  before `temper serve standalone` starts.
- `logs/runner.log` — real Forgejo Actions runner job execution.
- `logs/forgejo.log` — server-side errors from the throwaway Forgejo.

## Minimal movement trail

For the fixed single-repo run, expect this shape in `logs/run.log`:

```text
trigger: webhook listener up on 127.0.0.1:38200/forgejo/webhook (issue, PR, CI events)
worker:  capacity: architect=1 engineer=1 reviewer=1 (per-role, shared across all repos)
engine:  ready -- watching acme/service, idle
trigger: [acme/service#1] wake | artifact=intake queue=raw_intake ... event="wake.received"
engine:  [acme/service#1] mark_untriaged applied | +untriaged
agent:   [acme/service#1] architect/triage start ... event="agent.started"
agent:   [acme/service#1] architect/triage done ... event="agent.finished"
agent:   [acme/service#1] engineer/coding start ...
engine:  [acme/service PR#2] opened ... event="pr.opened"
agent:   [acme/service#2] reviewer/review start ...
agent:   [acme/service#2] reviewer/review done ...
engine:  [acme/service PR#2] merged -> main ... event="pr.merged"
engine:  [acme/service#1] resolved -- implemented by PR#2 ... event="item.resolved"
```

The seed-last webhook-wake proof is the `event="wake.received"` line followed by
`mark_untriaged applied`: intake is filed only after standalone is ready, so its
creation webhook drives the first transition rather than the slow poll backstop.

## Validator

Run while the demo is alive or after the backstop tears it down:

```sh
./run.sh validate
```

The validator checks that the webhook was registered, standalone reached
readiness, a webhook wake was accepted, architect/engineer/reviewer all ran, an
implementation PR merged, the source issue resolved, and Forgejo 7.0.x CI-read
fallback credentials were not missing or rejected.
