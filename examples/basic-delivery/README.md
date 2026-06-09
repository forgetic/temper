# Basic-delivery example

A **deliberately minimal**, no-human-in-the-loop Temper demo: it drives a single
issue **from submission to a merged PR with nobody in the loop**, using
deterministic fake agents. It is the "happy path, nothing fancy" counterpart to
[`examples/reference-delivery/`](../reference-delivery/): **one** repo, **three**
roles (`architect`, `engineer`, and the `bot` mechanical automation authority)
plus CI, webhooks on, and landing gated on **CI alone** — no reviewer, no owner,
no human, and no Smith/LLM dependency.

Like reference-delivery it boots a throwaway Forgejo server, registers a real
host-mode `forgejo-runner`, and runs `temper-testing-worker` (one OS process per
role plus one mechanical reconciler) against the real Forgejo backend. Unlike
reference-delivery it loads its own **3-role spec at runtime** (`--workflow
config/workflow.json`) and seeds the intake issue as the **site admin** — there
is no `human` role and no cross-repo fan-out.

## What this example proves

- Forgejo is the durable workflow state store: issues, labels, PRs, CI status,
  and metadata blocks carry state.
- CI is real: the bundled `forgejo-runner` runs the checked-in host-mode workflow
  on this machine.
- Role behavior is fake but process-isolated: `temper-testing-worker --profile
  basic` runs the basic-delivery architect + engineer fakes (no reviewer / owner
  / human), plus one mechanical reconciler, against the real Forgejo backend.
- Webhooks are real wake hints: the production `temper-trigger-forgejo` receives
  Forgejo webhooks and wakes the local fake workers; polling remains the
  correctness backstop.

## Expected flow

The intake flows end to end with only three workers running:

1. `run.sh` boots a throwaway Forgejo + runner and provisions exactly **one org +
   repo** (`acme/service` by default) — labels, CI, role users/tokens, and the
   webhook — passing `--workflow config/workflow.json` so the bundled 3-role spec
   applies. Because that spec declares `intake_author: { "kind": "site_admin" }`,
   provisioning seeds **one unlabeled intake issue authored by the site admin**
   (the "external filer").
2. The fake **bot** (`mechanical` worker) stamps the raw intake `untriaged` (the
   `raw_intake` queue's `mark_untriaged` automation).
3. The fake **architect** triages it into a `code` + `ready` issue with a crisp
   body. `triage_intake` has a single `ready_code → triage_intake_to_code`
   outcome — no design/breakdown branch, no fan-out.
4. The fake **engineer** claims the ready code issue and opens an
   `implementation` PR via a single `open_pr` transition (its head carries the
   `[ci-pass]` marker when `FAKE_CI_SENTINEL=present`). With no review gate, the
   PR drops straight into the `landing` queue.
5. The real **`forgejo-runner`** runs CI on the PR head and it goes green.
6. The **bot** sees the green PR and **auto-merges** it (the `landing` queue's
   `land_pr` automation, gated on `ci_gate` only), then marks it `landed`.

No reviewer approves; no owner or human acts. The bot is the **sole landing
authority** and lands purely on CI.

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
- `temper-testing-worker`

Set `TEMPER_SKIP_BUILD=1` only when those paths are already current.

## Layout

```text
examples/basic-delivery/
├── README.md
├── observability.md
├── config/
│   ├── temper.env       # non-secret knobs
│   ├── workflow.json    # the 3-role basic-delivery spec (tracks the canonical
│   │                    #   fixture crates/temper-workflow/fixtures/
│   │                    #   basic-delivery.json — keep the two in sync; see below)
│   └── ci.yml           # host-mode marker CI workflow (the [ci-pass] gate the
│                        #   fake engineer satisfies)
├── secrets/
│   └── .env.example     # optional local override template
└── run.sh               # launcher / teardown / validators
```

Runtime data goes under gitignored `run/`, `logs/`, and `secrets/roles.env`.

> **Keeping the spec in sync.** `config/workflow.json` is a **byte-for-byte copy**
> of the canonical fixture `crates/temper-workflow/fixtures/basic-delivery.json`
> (validation/route tests in `crates/temper-workflow/tests/basic_delivery.rs`).
> The two must stay identical; the test
> `crates/temper-testing/tests/basic_delivery_launcher_static.rs` asserts byte
> equality, so a drift fails CI.

## Temper prerequisite

This example loads its own 3-role spec at runtime and seeds intake as the site
admin, so it depends on three child issues of #61:

- **W1 — runtime workflow selection (#63).** `temper-testing-worker` and
  `temper-provision-forgejo` accept `--workflow <path>` (and
  `TEMPER_WORKFLOW_FILE`), defaulting to the bundled reference fixture when unset.
  `run.sh` passes `--workflow config/workflow.json` to **both** the provision and
  the worker invocations.
- **W2 — basic-delivery fake agents (#62).** The worker accepts `--profile
  <reference|basic>`; `basic` selects the architect + engineer fakes whose
  transitions match this workflow (single `triage_intake_to_code` / `open_pr`
  rather than the reference fan-out). `run.sh` passes `--profile basic` to the
  role workers.
- **W3 — `intake_author` site admin (#65).** `config/workflow.json` declares
  `intake_author: { "kind": "site_admin" }`, so provisioning seeds the intake
  issue as the setup-only admin rather than a `human` role (this workflow has
  none).

## Quick start

From this directory:

```sh
POLL_MS=120000 ./run.sh start     # blocks until Ctrl-C, ./run.sh stop, or RUN_SECS
./run.sh validate-webhooks        # while the run is still alive (or after)
./run.sh stop
```

Progress is printed without secrets (server URL, the seeded issue URL, where logs
live); per-process logs land under `logs/`. The checked-in default
`POLL_MS=120000` is intentional: polling is only the liveness backstop, while the
trigger's webhook wakes make the demo visibly progress before the two-minute
deadline. The default `BASE_URL` (`http://127.0.0.1:4100`) and `TRIGGER_BIND`
(`127.0.0.1:38090`) are **distinct from reference-delivery** (`4000` / `38080`),
so both demos can run side by side.

## Useful knobs

Edit `config/temper.env` or export variables before launch:

- `OWNER=acme` / `NAME=service` — the single repo provisioned and scanned.
- `POLL_MS=120000` — long-poll mode for role workers; webhooks should wake
  workers promptly.
- `CI_STATUS_POLL_MS=30000` — slow mechanical landing/CI-status missed-event
  backstop; leave blank to reuse `POLL_MS`. CI/PR webhooks and targeted wakes
  provide prompt landing/CI reactivity without shortening role long-poll mode.
- `IDLE_POLL_MAX_MS=8000` — cap for adaptive mechanical idle backoff.
- `WEBHOOKS=1|0` — enable/disable the local webhook trigger.
- `FAKE_CI_SENTINEL=present|deferred` — make the first PR head's CI pass, or force
  a fail-then-pass repair path through the engineer's `address_ci_failure`.

## Validation and troubleshooting

Run the validator before teardown; `./run.sh stop` removes the throwaway Forgejo
state.

```sh
./run.sh validate-webhooks
```

It checks provisioning logs (webhook registration, the site-admin intake URL),
webhook delivery/wake logs, fake worker tick logs, and mechanical CI-read
diagnostics. See `observability.md` for log names and expected event trails. A PR
stuck with `implementation` and never `landed` usually lacks current-head CI; if
`logs/mechanical.log` mentions missing web-UI credentials for the CI read
fallback, the mechanical worker was not launched with the provisioned `bot`
username/password (ADR 0019).

If a run is force-killed, clean up possible orphans:

```sh
pkill -f forgejo
pkill -f forgejo-runner
pkill -f temper-testing-worker
pkill -f temper-trigger-forgejo
rm -rf examples/basic-delivery/run
```

## Point it at your own Forgejo

Set `BASE_URL` to your instance and provide tokens, then drop the bundled
server/runner + provisioning steps — the same "swap to real" story as
reference-delivery. The workflow spec, the CI-only landing gate, and the
single-outcome triage are unchanged. A real project replaces `config/ci.yml` with
its real CI (build, test, lint) and pairs the engineer role with a coding agent
whose diffs pass it.
