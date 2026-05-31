# Reference-delivery example

A **self-contained, runnable demo** of the harness's production topology: a
Forgejo server, a `forgejo-runner` producing real CI, and one LLM-backed worker
per workflow role, all coordinating through the Forge to drive the bundled
reference-delivery workflow from an intake issue to a merged, reconciled PR.

> **Status:** complete and validated. `run.sh` boots and tears down every
> process, and a real end-to-end run (ChatGPT OAuth agents, real CI) converged
> the intake issue to a merged, reconciled PR in ~22 s on a 4-core host. The
> same convergence is asserted by the gated test
> `crates/harness-testing/tests/forgejo_multiprocess.rs`.

## Honest framing — read this first

This is a **demo**, not a turnkey production deployment:

- It boots its **own throwaway Forgejo + runner** so it runs from binaries
  alone. To target a **real** Forgejo you change `BASE_URL` + tokens and drop the
  bundled server/runner + provisioning — the same "swap to real" story as
  [`docs/how-to/run-forgejo-multiprocess-e2e.md`](../../docs/how-to/run-forgejo-multiprocess-e2e.md).
- CI converges because the bundled `config/ci.yml` gates on a simple
  **commit-message marker** the engineer emits — there is no real
  toolchain/checkout. A real project swaps in its real CI and a real coding
  agent.
- It is the operator-runnable, shell-driven version of the gated test
  `crates/harness-testing/tests/forgejo_multiprocess.rs` — not new behavior.

Keep these caveats in mind; this does not pretend to be more than a faithful
end-to-end rehearsal.

## Prerequisites

- The workspace binaries built: `cargo build --release -p harness-testing`
  (provides `harness-testing-worker` and `harness-testing-provision`). `run.sh`
  builds them on first run unless `HARNESS_SKIP_BUILD=1`.
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
  - **DeepSeek key (fallback, bills per token):** set
    `HARNESS_AGENTS_AUTH=deepseek` and provide `secrets/deepseek-api-key`.

See `secrets/.env.example` for both options in detail.

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
./run.sh          # boot Forgejo + runner, provision + seed, launch the workers,
                  #   then block; Ctrl-C tears everything down
./run.sh stop     # tear down a previous run via the saved PIDs
./run.sh help     # usage
```

The first run builds the workspace binaries (`cargo build --release -p
harness-testing`) if they are missing and expects the pinned Forgejo +
`forgejo-runner` binaries under `.cache/forgejo/` (or set
`HARNESS_FORGEJO_BINARY` / `HARNESS_FORGEJO_RUNNER_BINARY`). Edit
`config/harness.env` for the repo, endpoint, cadence, and auth knobs; any of
those may also be overridden by exporting the matching env var before invoking
the script (env wins over the file).

Progress is printed without secrets (server URL, the seeded issue URL, where
logs live); per-process logs land under `logs/`.

## What it does

Boots Forgejo + a host-mode `forgejo-runner`, provisions an org/repo with a
per-role user + token (and seeds one intake issue), then launches one
`harness-testing-worker --backend forgejo --agents real --clock wall` per
role-with-an-agent plus one mechanical reconciler.

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

## Watching progress

Open the Forgejo UI at `BASE_URL` (log in as any provisioned role). Watch the
issue get triaged, the PR open, CI run, the review land, and the merge +
reconcile labels move. Worker logs land under `logs/` (created at run time).

## Teardown

`./run.sh stop` (or a `SIGINT`/`SIGTERM` to the running script) kills every spawned
process and removes the throwaway data dirs. If a run is force-killed, clean up
orphans with `pkill -f forgejo` / `pkill -f harness-testing-worker` and remove
the run/data dirs.

## Point it at your own Forgejo

Change `BASE_URL` to your instance, supply real per-role tokens instead of the
generated ones, and skip the bundled server/runner + provisioning steps. Replace
`config/ci.yml` with your real CI and pair the engineer with a real coding agent.
The one remaining "swap to real" item is **webhook triggering** (an
ADR 0009 `ChangeHint` accelerator alongside the level-triggered `PollLoop`).
This is the same swap-to-real path documented in
[`docs/how-to/run-forgejo-multiprocess-e2e.md`](../../docs/how-to/run-forgejo-multiprocess-e2e.md)
and [`docs/explanation/forgejo-e2e-topology.md`](../../docs/explanation/forgejo-e2e-topology.md).
