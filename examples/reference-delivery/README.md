# Reference-delivery example

A **self-contained Temper demo**: it boots a throwaway Forgejo server, registers
a real host-mode `forgejo-runner`, and drives the reference-delivery workflow
with deterministic fake agent worker binaries.

## What this example proves

- Forgejo is the durable workflow state store: issues, labels, PRs, reviews,
  dependencies, CI status, and metadata blocks carry state.
- CI is real: the bundled `forgejo-runner` runs the checked-in host-mode workflow
  on this machine.
- Role behavior is fake but process-isolated: `temper-testing-worker` runs one
  OS process per role plus one mechanical reconciler against the real Forgejo
  backend.
- Webhooks are real wake hints: the production `temper-trigger-forgejo` receives
  Forgejo webhooks and wakes the local fake workers; polling remains the
  correctness backstop.

## Prerequisites

- Rust workspace build tools.
- The pinned Forgejo `7.0.12` and `forgejo-runner` `3.5.1` binaries, either in
  the shared `.cache/forgejo/` cache or through `TEMPER_FORGEJO_BINARY` and
  `TEMPER_FORGEJO_RUNNER_BINARY` in `config/temper.env` or the environment.
  Ignored Forgejo fixture tests fill the shared cache automatically on first
  startup when network access is available.
- A host where the runner may execute host-mode jobs directly. No containers are
  used.

`run.sh` builds the needed development-profile binaries by default:

- `temper-provision-forgejo`
- `temper-trigger-forgejo`
- `temper-validate-reference-delivery`
- `temper-testing-worker`

Set `TEMPER_SKIP_BUILD=1` only when those paths are already current.

## Layout

```text
examples/reference-delivery/
├── README.md
├── observability.md
├── config/
│   ├── temper.env       # non-secret knobs
│   ├── workflow.json    # reference-delivery workflow copy
│   └── ci.yml           # host-mode CI workflow
├── secrets/
│   └── .env.example     # optional local override template
└── run.sh               # launcher / teardown / validators
```

Runtime data goes under gitignored `run/`, `logs/`, and `secrets/roles.env`.

## Quick start

From this directory:

```sh
POLL_MS=120000 ./run.sh start     # blocks until Ctrl-C, ./run.sh stop, or RUN_SECS
./run.sh validate-multi-repo      # while the run is still alive
./run.sh validate-webhooks
./run.sh stop
```

The default repo set is:

```sh
REPOS="acme/service acme/service-canary"
CROSS_REPO_INTAKE=auto
```

That seeds one parent intake in `acme/service`. The hidden fake-architect plan in
that issue asks the fake architect to create one child code issue per repo.

## Expected flow

1. The launcher provisions each repo with labels, CI, role users/tokens, and a
   repo webhook.
2. The source repo receives one parent intake issue.
3. The fake architect fans it out into one child code issue per configured repo
   and blocks the parent on those children.
4. Fake engineers create real Forgejo PR heads and open implementation PRs.
5. The real `forgejo-runner` runs CI for each PR head.
6. Fake reviewers approve PRs.
7. The mechanical worker lands PRs after reviewer approval and current-head CI.
   It runs as the provisioned `bot` automation user: the bot's token performs
   REST mutations (merges) and the bot's web-UI login reads Forgejo 7.0.x CI
   status (ADR 0019). The setup-only site admin is not involved, and the owner
   role does not perform normal merges.
8. If a merge conflict is reported, mechanical automation removes `landing`, adds
   `merge-conflict`, and the fake engineer requeues after updating the PR head;
   fresh CI is still required, but a second review is not.
9. The closing fake architect reconciles landed PRs and closes produced code
   issues so the mechanical worker can unblock the parent dependency aggregate.
10. Fake owners later handle alignment cohorts when the queue activation policy
   reaches depth or age.

## Useful knobs

Edit `config/temper.env` or export variables before launch:

- `REPOS="owner/a owner/b"` — fixed repo set scanned by every worker.
- `CROSS_REPO_INTAKE=auto|1|0` — one parent fan-out issue or independent intakes.
- `POLL_MS=120000` — long-poll mode for role workers; webhooks should wake
  workers promptly.
- `CI_STATUS_POLL_MS=1000` — narrow active mechanical landing/CI-status poll
  backstop; leave blank to reuse `POLL_MS`. Forgejo 7.0.x does not emit
  Actions-completion webhooks, so this keeps green approved PRs moving without
  shortening role long-poll mode.
- `IDLE_POLL_MAX_MS=8000` — cap for adaptive mechanical idle backoff after
  repeated no-action normal ticks. Wakes still tick immediately, and any
  progress, wake, audit, or error resets the next normal poll to
  `CI_STATUS_POLL_MS`.
- `WEBHOOKS=1|0` — enable/disable local webhook trigger.
- `FAKE_ARCHITECT=closing|default` — `closing` is best for cross-repo convergence.
- `FAKE_REVIEWER=default|request-changes-then-approve`.
- `FAKE_CI_SENTINEL=present|deferred` — make first PR CI pass, or force a
  fail-then-pass repair path.

## Validation and troubleshooting

Run validators before teardown; `./run.sh stop` removes the throwaway Forgejo
state.

```sh
./run.sh validate-webhooks
./run.sh validate-multi-repo
```

The multi-repo validator checks provisioning logs, webhook delivery/wake logs,
fake worker logs, mechanical CI-read diagnostics, and live Forge state for the
parent/child dependency shape. See `observability.md` for log names and expected
event trails, including `mechanical_automation_*` entries for landing and
conflict routing. Persistent `wake_delivery outcome=no_sockets` usually means a
worker failed before binding its wake socket or the first-handoff worker ran
before downstream sockets were ready; inspect the matching fake worker log. A PR
stuck with `landing` usually lacks current-head CI or reviewer approval; if
`logs/mechanical.log` mentions missing web-UI credentials for the CI read
fallback, the mechanical worker was not launched with the provisioned `bot`
username/password. A PR stuck with `merge-conflict` needs an engineer
conflict-resolution push before mechanical landing retries.

If a run is force-killed, clean up possible orphans:

```sh
pkill -f forgejo
pkill -f forgejo-runner
pkill -f temper-testing-worker
pkill -f temper-trigger-forgejo
rm -rf examples/reference-delivery/run
```

## Related checks

Hermetic/default-process coverage:

```sh
cargo test -p temper-testing --test multi_repo_multiprocess
```

Live Forgejo + runner fake-agent fixture:

```sh
cargo test -p temper-testing --test forgejo_multiprocess -- --ignored
cargo test -p temper-testing --test forgejo_multi_repo_webhook -- --ignored
cargo test -p temper-testing --test forgejo_webhook_wakeup -- --ignored
```

Add `--test-threads=1` only as an optional host resource throttle; Forgejo
fixture caches and runtime paths are parallel-safe.
