# Reference-delivery example

A **self-contained Temper topology demo** that has no Smith or provider-auth
requirement: it boots a throwaway Forgejo server, registers a real host-mode
`forgejo-runner`, and drives the reference-delivery workflow with deterministic
fake agent worker binaries.

The Smith-backed operator/dogfood examples that used to live under Temper's
`examples/` tree have been copied to `~/src/rust/smith/examples/`. Temper keeps
this example as the hermetic Forgejo + runner rehearsal.

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

This is not a production deployment and not an LLM demo. Use Smith for
provider-backed role decisions and product-chat examples.

## Prerequisites

- Rust workspace build tools.
- The pinned Forgejo `7.0.12` and `forgejo-runner` `3.5.1` binaries. Populate the
  cache with:

  ```sh
  cargo test -p temper-forgejo-fixture --test cache -- --ignored
  ```

  Or set `TEMPER_FORGEJO_BINARY` and `TEMPER_FORGEJO_RUNNER_BINARY` in
  `config/temper.env` or the environment.
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
7. Fake owners merge after CI and review gates pass.
8. The closing fake architect reconciles landed PRs and closes produced code
   issues so the mechanical worker can unblock the parent dependency aggregate.

## Useful knobs

Edit `config/temper.env` or export variables before launch:

- `REPOS="owner/a owner/b"` — fixed repo set scanned by every worker.
- `CROSS_REPO_INTAKE=auto|1|0` — one parent fan-out issue or independent intakes.
- `POLL_MS=120000` — long-poll mode; webhooks should wake workers promptly.
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
fake worker logs, and live Forge state for the parent/child dependency shape.
See `observability.md` for log names and expected event trails. Persistent
`wake_delivery outcome=no_sockets` usually means a worker failed before binding
its wake socket; inspect the matching fake worker log.

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
cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --test-threads=1
cargo test -p temper-testing --test forgejo_multi_repo_webhook -- --ignored --test-threads=1
```
