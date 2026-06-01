# Reference-delivery example

A **self-contained operator demo** of the harness's production topology: a
Forgejo server, a `forgejo-runner` producing real CI, and one LLM-backed worker
per workflow role, all coordinating through the Forge to drive the bundled
reference-delivery workflow from an intake issue to a merged, reconciled PR.

> **Status:** this example is wired to production-owned binary names
> (`harness-worker`, `harness-provision-forgejo`, and
> `harness-trigger-forgejo`) from the `harness-production` crate instead of the
> `harness-testing` binaries. It has
> been **revalidated live end-to-end** (the happy path converged to a merged,
> reconciled PR against the bundled Forgejo + real CI, driven by real LLM agents
> on the Anthropic OAuth `claude-opus-4-8` auth mode). Two bugs surfaced and
> were fixed during that run — the Anthropic OAuth Claude Code system identity
> (agent lesson 0012) and the provisioner's CI workflow YAML losing its
> indentation to `\`-continued string literals (agent lesson 0013). See
> [`plans/production-binaries/`](../../plans/production-binaries/README.md).
> The topology was also previously validated by the gated Forgejo multi-process
> test and the earlier testing-binary launcher.

## Honest framing — read this first

This is a **demo**, not a turnkey production deployment:

- It uses the production-owned binaries and does not fall back to
  `harness-testing` entry points. If those binaries are absent, `run.sh` stops at
  the build/resolve step.
- It boots its **own throwaway Forgejo + runner** so it runs from binaries
  alone. To target a **real** Forgejo you change `BASE_URL` + tokens and drop the
  bundled server/runner + provisioning — the same "swap to real" story as
  [`docs/how-to/run-forgejo-multiprocess-e2e.md`](../../docs/how-to/run-forgejo-multiprocess-e2e.md).
- CI converges because the bundled `config/ci.yml` gates on a simple
  **commit-message marker** the engineer emits — there is no real
  toolchain/checkout. A real project swaps in its real CI and a real coding
  agent.
- It is the operator-facing, shell-driven version of the same topology covered
  by the gated Forgejo multi-process test — not new workflow behavior.

Keep these caveats in mind; this does not pretend to be more than a faithful
end-to-end rehearsal.

## Prerequisites

- The production workspace binaries built: `cargo build --release -p
  harness-production` (provides `harness-worker`,
  `harness-provision-forgejo`, and `harness-trigger-forgejo`). `run.sh` builds
  them on first run unless `HARNESS_SKIP_BUILD=1`. Override paths with
  `HARNESS_WORKER_BIN` / `HARNESS_PROVISION_BIN` / `HARNESS_TRIGGER_BIN` if
  needed.
- The two pinned binaries: Forgejo `7.0.12` and `forgejo-runner` `3.5.1`. Let
  the first run download + checksum them into `.cache/forgejo/`, or pre-stage
  them and set `HARNESS_FORGEJO_BINARY` / `HARNESS_FORGEJO_RUNNER_BINARY` in
  `config/harness.env`.
- A host that permits **host-mode** CI jobs (spawning child processes, binding a
  loopback port) — the runner executes steps directly on the host, no containers.
- LLM agent auth — **one of**:
  - **ChatGPT login (preferred, no per-call cost):** `pi /login openai-codex`
    once, which populates `~/.pi/agent/auth.json`. This is the default
    (`HARNESS_AGENTS_AUTH=chatgpt-oauth`).
  - **Anthropic OAuth (opt-in):** `pi /login anthropic` once, then set
    `HARNESS_AGENTS_AUTH=anthropic-oauth`.
  - **DeepSeek key (fallback, bills per token):** set
    `HARNESS_AGENTS_AUTH=deepseek` and provide `secrets/deepseek-api-key`.

See `secrets/.env.example` for the options in detail.

## Layout

```text
examples/reference-delivery/
├── README.md            # this file
├── .gitignore           # ignores runtime run/, logs/, *.pid, *.log
├── config/
│   ├── harness.env      # operator-editable knobs (no secrets)
│   ├── workflow.json    # the bundled workflow spec (tracks the canonical
│   │                    #   fixture crates/harness-workflow/fixtures/
│   │                    #   reference-delivery.json — do not fork its semantics)
│   └── ci.yml           # the host-mode CI workflow (commit-message marker)
├── secrets/             # gitignored except the templates + .gitignore
│   ├── .env.example
│   └── deepseek-api-key.example
└── run.sh               # launcher/teardown (phase B3)
```

The workflow **roles** (architect, engineer, reviewer, owner, human) and the
labels are **not** configured by hand — the workers derive them from
`config/workflow.json` (the compiled workflow ∩ the role bindings). `config/`
only carries what an operator must edit (repo, endpoint, cadence, auth).

## Quick start

From this directory:

```sh
POLL_MS=120000 ./run.sh   # long-poll mode: webhooks wake workers promptly;
                           #   Ctrl-C tears everything down
./run.sh validate-webhooks # summarize webhook registration/delivery/wake logs
./run.sh stop              # tear down a previous run via the saved PIDs
./run.sh help              # usage
```

The first run builds the production workspace binaries (`cargo build --release
-p harness-production`) if they are missing and expects the pinned Forgejo +
`forgejo-runner` binaries under `.cache/forgejo/` (or set
`HARNESS_FORGEJO_BINARY` / `HARNESS_FORGEJO_RUNNER_BINARY`). Edit
`config/harness.env` for the repo, endpoint, cadence, and auth knobs; any of
those may also be overridden by exporting the matching env var before invoking
the script (env wins over the file).

Progress is printed without secrets (server URL, the seeded issue URL, where
logs live); per-process logs land under `logs/`. The checked-in default
`POLL_MS=120000` is intentional: polling is only the liveness backstop, while
webhooks should make the demo visibly progress before the two-minute deadline.

## What it does

Boots Forgejo + a host-mode `forgejo-runner`, starts the local webhook trigger,
provisions an org/repo with a per-role user + token (and seeds one intake issue),
then launches one `harness-worker` per role-with-an-agent plus one mechanical
reconciler. Workers still use wall-clock polling as the liveness backstop;
webhooks only wake them early.

The seeded intake issue then flows through the reference-delivery workflow, each
step driven by the role worker whose LLM **decides** and then mutates workflow
state **only through `RoleTools`** (the same narrow boundary the deterministic
fakes use):

1. **architect** triages the intake issue into a `code` issue;
2. **engineer** claims it (`in-progress`), prepares a real head branch, and opens
   an implementation PR;
3. the **`forgejo-runner`** runs real CI on the PR head;
4. **reviewer** approves;
5. **owner** merges once CI + review gates are green;
6. post-merge, the **architect/owner** reconcile the routing labels — `landed`
   cleared, `alignment` retained — reaching the quiescent end state.

The **mechanical** worker runs the controller plane (lease expiry, partial-
transition repair, dependency unblock) without an agent. See
[`docs/explanation/forgejo-e2e-topology.md`](../../docs/explanation/forgejo-e2e-topology.md)
for the durable topology and real-CI design.

## Watching progress and validating webhooks

Open the Forgejo UI at `BASE_URL` (log in as any provisioned role). Watch the
issue get triaged, the PR open, CI run, the review land, and the merge +
reconcile labels move. Worker logs land under `logs/` (created at run time).
With webhooks enabled:

- `logs/provision.log` records `webhook registered url=...` after the provisioner
  successfully registered the repo hook;
- `logs/trigger.log` reports `listening on`, `webhook accepted` or
  `webhook rejected`, and `wake_delivery outcome=no_sockets|sent|all_failed`
  with target/sent/failed counts;
- worker logs report `consumed authenticated wake` and then
  `completed tick trigger=wake actions=N`. `actions=0` means the worker woke,
  scanned fresh Forge state, and found no queue item.

In another terminal, run:

```sh
./run.sh validate-webhooks
```

It summarizes accepted webhook deliveries, wake batches sent, per-worker wake
consumption, wake-triggered ticks, and whether any wake-triggered tick made
workflow progress (`actions>0`). For a long-poll smoke, start the demo with
`POLL_MS=120000 ./run.sh`, wait until some workflow movement appears in Forgejo,
then run `./run.sh validate-webhooks`; it should pass before any two-minute poll
backstop is needed.

## Troubleshooting long-poll wakeups

- **No `webhook registered` in `logs/provision.log`:** provisioning failed before
  the hook was registered, or `WEBHOOKS=0` was set.
- **Registered but no `webhook accepted` in `logs/trigger.log`:** confirm the
  trigger reached `listening on`, `WEBHOOK_URL` points at `TRIGGER_BIND`, and the
  bundled Forgejo config allows loopback webhooks (`ALLOWED_HOST_LIST` includes
  `127.0.0.1,localhost`).
- **Accepted but `wake_delivery outcome=no_sockets`:** a webhook arrived before
  workers created wake sockets. The launcher starts downstream workers and waits
  for their sockets before launching the architect, so persistent `no_sockets`
  usually means workers failed during startup; inspect the role logs.
- **Wakes sent but a worker lacks `consumed authenticated wake`:** inspect that
  worker's log for auth/backend setup errors. Polling will still recover at the
  next `POLL_MS` deadline, but the accelerator is not working for that worker.
- **Logs do not contain the strings above:** rebuild the production binaries
  (`cargo build --release -p harness-production`) or leave `HARNESS_SKIP_BUILD`
  unset. `HARNESS_SKIP_BUILD=1` assumes `target/release` is already current.
- **Wake consumed with `actions=0`:** the wake path worked; that worker simply
  had no active queue item after re-reading Forge state.

## Teardown

`./run.sh stop` (or a `SIGINT`/`SIGTERM` to the running script) kills every spawned
process and removes the throwaway data dirs. If a run is force-killed, clean up
orphans with `pkill -f forgejo` / `pkill -f harness-worker` and remove
the run/data dirs.

## Point it at your own Forgejo

Change `BASE_URL` to your instance, supply real per-role tokens instead of the
generated ones, and skip the bundled server/runner + provisioning steps. Replace
`config/ci.yml` with your real CI and pair the engineer with a real coding agent.
The demo now includes the ADR 0009 **webhook triggering** accelerator for the
local topology. For a real Forgejo, expose `WEBHOOK_URL` over HTTPS to the Forgejo
server and keep the worker wake sockets host-local. This is the same
swap-to-real path documented in
[`docs/how-to/run-forgejo-multiprocess-e2e.md`](../../docs/how-to/run-forgejo-multiprocess-e2e.md)
and [`docs/explanation/forgejo-e2e-topology.md`](../../docs/explanation/forgejo-e2e-topology.md).
