# Basic-delivery example

A **deliberately minimal**, no-human-in-the-loop Temper demo: it drives a single,
thin intake issue **from submission to a merged PR with nobody in the loop**,
using the **real in-process coding agent**. It is the "happy path, nothing fancy"
counterpart to [`examples/reference-delivery/`](../reference-delivery/): **one**
repo, the human-capable workflow roles `architect` and `engineer`, the `bot`
mechanical automation authority, CI, webhooks on from the start, and landing
gated on **CI alone** — no reviewer, no owner, no human.

It boots the production topology from development-profile binaries, but as a
**single process**: a throwaway Forgejo server, a host-mode `forgejo-runner`
producing real CI, `temper-provision-forgejo`, and **one `temper run`** — the
unified daemon + worker + coding agent on one event loop, serving the architect
and engineer roles for `acme/service`.

The proof this example exists to show is the thin-intake → architect-spec-rewrite
step: the filed issue says only what the filer wants, and the architect must turn
that into an implementable code spec before the engineer can write code.

## What it demonstrates

The run converges through the unified daemon/worker topology with a single
`temper run` process:

1. `run.sh` boots throwaway Forgejo + runner, then runs `temper-provision-forgejo`
   against `config/workflow.json` with `--seed-intake no`. That first pass creates
   exactly **one org + repo** (`acme/service` by default), users/tokens, labels,
   CI, and the webhook, but it deliberately does **not** file the intake issue yet.
2. `run.sh` starts **one `temper run`**. In one process it hosts the daemon (the
   Forgejo webhook route `POST /forgejo/webhook`, the long poll backstop, the
   short mechanical CI/merge backstop, leases, per-role apply tokens, result
   appliers), an in-process worker with capabilities for `acme/service:architect`
   and `acme/service:engineer`, and the in-process coding agent.
3. Only after `temper run` is ready (HTTP listener up, worker registered), `run.sh`
   uses the site-admin token to `POST /api/v1/repos/{owner}/{repo}/issues` and
   file **one unlabeled intake issue**. The issue is authored by the **site
   admin** because the workflow declares `intake_author: { "kind": "site_admin" }`;
   the direct API call mimics an external filer, not a workflow role.
4. Filing the issue last is the **seed-last webhook-wake proof**: the issue-created
   webhook reaches the daemon's `POST /forgejo/webhook` route, is accepted, and
   triggers a targeted wake scan instead of waiting for the long poll backstop.
5. The mechanical automation first stamps the raw intake `untriaged`, then the
   daemon assigns the architect a **triage job** under a lease (read-only
   checkout). The coding agent reads the issue + repo and returns
   `verdict=ready_code` with a rewritten body.
6. The daemon applies the `triage_intake_to_code` transition as the architect
   identity: `set_body` replaces the thin seed with the architect's complete spec,
   and the issue receives the `code` and `ready` labels.
7. The daemon assigns the engineer a writable coding job. The same in-process
   worker runs the agent in the engineer's persistent workspace, commits the
   product diff with a `Closes #<n>` trailer, and pushes the branch as the
   engineer, then opens an `implementation` PR. With no review gate, the PR drops
   straight into the `landing` queue.
8. The real **`forgejo-runner`** runs CI on the PR head and it goes green.
9. The **bot** (the mechanical backstop) sees the green PR and **auto-merges** it
   (the `landing` queue's `land_pr` automation, with no review gate). Forgejo
   closes the source issue via the merge trailer.

No reviewer approves; no owner or human acts. The bot is the **sole landing
authority** and lands purely on CI.

## What is real vs. canned

- **Real:** the Forgejo server, the host-mode `forgejo-runner` and its CI, the
  provisioning, the direct Forgejo issue-create API call, the daemon's Forge API
  authority (webhooks, leases, per-role apply tokens, mechanical landing), the
  worker's git workspaces, and the **coding agent itself** (architect triage +
  engineer implementation are real LLM work).
- **Canned:** only the thin seed intake body (`config/intake-issue.md`).

The coding agent's LLM provider is selected with `TEMPER_RUN_AUTH` in
`config/temper.env` (default `chatgpt-oauth`); provider credentials live in
`secrets/.env` (gitignored). See `secrets/.env.example`.

## Prerequisites

- Rust workspace build tools.
- `curl` for Forgejo readiness probes and Python 3 for the small Forgejo API JSON
  helper plus git credential URL encoding.
- The pinned Forgejo `7.0.12` and `forgejo-runner` `3.5.1` binaries, either in the
  shared `.cache/forgejo/` cache or through `TEMPER_FORGEJO_BINARY` and
  `TEMPER_FORGEJO_RUNNER_BINARY` in `config/temper.env` or the environment.
  Ignored Forgejo fixture tests fill the shared cache automatically on first
  startup when network access is available.
- A host where the runner may execute host-mode jobs directly. No containers.
- Provider credentials for the coding agent (see `secrets/.env.example`).

`run.sh` builds the needed development-profile binaries by default with
`cargo build -p temper` (the unified `temper` binary plus the provisioner). Set
`TEMPER_SKIP_BUILD=1` only when those paths are already current.

## Layout

```text
examples/basic-delivery/
├── README.md
├── config/
│   ├── temper.env       # non-secret knobs
│   ├── workflow.json    # the 3-role basic-delivery spec (tracks the canonical
│   │                    #   fixture crates/temper-workflow/fixtures/
│   │                    #   basic-delivery.json — keep the two in sync)
│   ├── intake-issue.md  # the deliberately thin seed intake body (intent only)
│   └── ci.yml           # host-mode pass-through CI (validates the engineer's
│                        #   real product diff: shell scripts must parse)
├── secrets/
│   └── .env.example     # provider-auth + local-override template
└── run.sh               # launcher / teardown / validators
```

Runtime data goes under gitignored `run/`, `logs/`, and `secrets/credentials.toml`
(the provisioned forge identities in the runtime's own credentials format, loaded
via `temper daemon --credentials`). The single process logs to `logs/run.log`.

> **Keeping the spec in sync.** `config/workflow.json` tracks the canonical fixture
> `crates/temper-workflow/fixtures/basic-delivery.json`. Keep the two identical; do
> not fork the spec's semantics here.

## Quick start

From this directory:

```sh
./run.sh start                  # blocks until Ctrl-C, ./run.sh stop, or RUN_SECS
./run.sh validate-webhooks      # while the run is still alive (or after)
./run.sh stop
```

Progress is printed without secrets (server URL, the seeded issue URL, where logs
live); the single process logs to `logs/run.log`. The default
`DAEMON_POLL_CADENCE_SECS=120` is intentional: polling is only the liveness
backstop, while the webhook wakes make the demo visibly progress before the
two-minute deadline. The default `BASE_URL` (`http://127.0.0.1:4100`) and
`DAEMON_BIND` (`127.0.0.1:38100`) are **distinct from reference-delivery**
(`4200` / `38200`), so both demos can run side by side.

## Useful knobs

Edit `config/temper.env` or export variables before launch:

- `OWNER=acme` / `NAME=service` — the single repo provisioned and scanned.
- `TEMPER_RUN_AUTH=chatgpt-oauth|anthropic-oauth|deepseek` — the coding agent's
  LLM provider/auth (passed as `temper run --auth`).
- `RUN_MAX_ITERATIONS=250` — max agent iterations per job.
- `DAEMON_POLL_CADENCE_SECS=120` — long poll backstop; webhooks drive prompt
  progress. **Do not shorten this** to compensate for webhooks — webhooks are the
  intended wake path.
- `DAEMON_MECHANICAL_CADENCE_SECS=2` — mechanical CI/landing reconciliation cadence
  (Forgejo 7.0.x has no Actions-completion webhook, so landing polls CI status).

## Validation and troubleshooting

Run the validator before teardown; `./run.sh stop` removes the throwaway Forgejo
state.

```sh
./run.sh validate-webhooks
```

It checks provisioning logs (webhook registration, the site-admin intake URL),
the single `logs/run.log` (serving readiness, webhook delivery + wake scan, daemon
assignments and results, the in-process worker's `worker:` register /
assign / result lines), and mechanical CI-read diagnostics. A PR stuck open with
`implementation` usually lacks current-head CI; if `logs/run.log` mentions
missing web-UI credentials for the CI read fallback, the
run was not launched with the provisioned `bot` username/password (ADR 0019).

If a run is force-killed, clean up possible orphans:

```sh
pkill -f forgejo
pkill -f forgejo-runner
pkill -f 'target/debug/temper'
rm -rf examples/basic-delivery/run
```

## Point it at your own Forgejo

Set `BASE_URL` to your instance and provide tokens, then drop the bundled
server/runner + provisioning steps. The workflow spec, the no-review landing
track, and the single-outcome triage are unchanged. A real project replaces
`config/ci.yml` with its real CI (build, test, lint); the engineer agent produces
diffs that pass it.
