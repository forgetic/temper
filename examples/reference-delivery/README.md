# Reference-delivery example

A **self-contained operator demo** of the harness's production topology: a
Forgejo server, a `forgejo-runner` producing real CI, and one LLM-backed worker
per workflow role scanning a configured repository set, all coordinating through
the Forge.

> **Status:** this example is wired to production-owned binary names
> (`harness-worker`, `harness-provision-forgejo`, and
> `harness-trigger-forgejo`) from the `harness-production` crate instead of the
> `harness-testing` binaries. After the user-defined-role migration, production
> workers register generic agents from compiled workflow manifests. Reference
> role behavior for this demo lives in `config/workflow.json` and the canonical
> workflow fixture, not in production prompt constants. Full intake-to-merged-PR
> convergence still requires a bound coding workspace that can produce real
> product diffs; the gated `harness-testing` Forgejo e2e fixtures cover the
> topology. See
> [`plans/production-binaries/`](../../plans/production-binaries/README.md) and
> [`plans/multi-repo-workers/`](../../plans/multi-repo-workers/README.md).

## Honest framing — read this first

This is a **demo**, not a turnkey production deployment:

- It uses the production-owned binaries and does not fall back to
  `harness-testing` entry points. If those binaries are absent, `run.sh` stops at
  the build/resolve step.
- It boots its **own throwaway Forgejo + runner** so it runs from binaries
  alone. To target a **real** Forgejo you change `BASE_URL` + tokens and drop the
  bundled server/runner + provisioning — the same "swap to real" story as
  [`docs/how-to/run-forgejo-multiprocess-e2e.md`](../../docs/how-to/run-forgejo-multiprocess-e2e.md).
- The bundled `config/ci.yml` still demonstrates the old commit-message marker
  shape, but production role workers no longer synthesize PR-head commits. A real
  project must bind a real coding workspace before engineer automation can open
  meaningful PRs; the PR diff guard rejects bookkeeping-only heads.
- It is the operator-facing, shell-driven version of the same topology covered
  by the gated Forgejo multi-process tests — not new workflow behavior.
- Cross-repo fan-out is planning and aggregation only. It does **not** add atomic
  cross-repo merges, shared branches, per-repo workflow definitions, or fairness
  guarantees beyond the fixed worker scan set.

Keep these caveats in mind; this does not pretend to be more than a faithful
end-to-end rehearsal.

## Prerequisites

- The operator-facing workspace binaries built: `cargo build -p
  harness-production` (provides `harness-worker`,
  `harness-provision-forgejo`, and `harness-trigger-forgejo`). `run.sh` refreshes
  the development-profile binaries under `target/debug` before start unless
  `HARNESS_SKIP_BUILD=1`, so stale binaries do not break the demo after source
  changes. Override paths with `HARNESS_WORKER_BIN` / `HARNESS_PROVISION_BIN` /
  `HARNESS_TRIGGER_BIN` if needed.
- The two pinned binaries: Forgejo `7.0.12` and `forgejo-runner` `3.5.1`.
  Pre-stage them under `.cache/forgejo/` (the gated Forgejo e2e fixtures do the
  download/checksum path) or set `HARNESS_FORGEJO_BINARY` /
  `HARNESS_FORGEJO_RUNNER_BINARY` in `config/harness.env`.
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

The workflow **roles** (architect, engineer, reviewer, owner, human), labels,
role guidance, prompt extensions, and external-tool declarations are derived from
`config/workflow.json` (which tracks the canonical fixture). Generated prompts
carry mechanics and authority boundaries; `charter`, `prompt.guidance`,
`prompt.tool_guidance`, and `external_tools` carry the reference-delivery demo's
user-authored behavior. `config/` otherwise carries what an operator must edit
(repo, endpoint, cadence, auth, coding workspace binding, and whether the demo
seeds one cross-repo parent intake).

## Quick start

From this directory:

```sh
POLL_MS=120000 ./run.sh       # long-poll mode: webhooks wake workers promptly;
                               #   Ctrl-C tears everything down
./run.sh validate-multi-repo   # repo-specific provisioning/webhook/worker smoke
./run.sh validate-webhooks     # summarize webhook registration/delivery/wake logs
./run.sh stop                  # tear down a previous run via the saved PIDs
./run.sh help                  # usage
```

Each start refreshes the development-profile workspace binaries (`cargo build -p
harness-production`, usually a no-op when current) under `target/debug` and
expects the pinned Forgejo + `forgejo-runner` binaries under `.cache/forgejo/`
(or set `HARNESS_FORGEJO_BINARY` / `HARNESS_FORGEJO_RUNNER_BINARY`). `run.sh
start` runs from a private snapshot under `run/`, so editing the launcher while a
demo is running cannot corrupt the eventual teardown path. Edit
`config/harness.env` for the repo set, endpoint, cadence, and auth knobs; any of
those may also be overridden by exporting the matching env var before invoking
the script (env wins over the file). The checked-in default is
`REPOS="acme/service acme/service-canary"` with `CROSS_REPO_INTAKE=auto`, so the
first repo receives one parent intake that asks the architect to create children
in both repos. Set `CROSS_REPO_INTAKE=0` for independent per-repo intakes, or
set `REPOS=` and `CROSS_REPO_INTAKE=0` to preserve the single-repo `OWNER`/`NAME`
mode. Every production worker scans the same repository set (tokens must have
Forge access to every listed repo; Forge permissions, not scan-shard membership,
authorize writes and child creation).

Progress is printed without secrets (server URL, seeded issue URLs, where logs
live); per-process logs land under `logs/`. The checked-in default
`POLL_MS=120000` is intentional: polling is only the liveness backstop, while
webhooks should make the demo visibly progress before the two-minute deadline.

## Coding workspace binding

The engineer role declares `coding_workspace` in `config/workflow.json`, but the
tool is unavailable until the runner binds a provider. Leave the binding empty to
show the safe idle state: the generated prompt lists no bound external tools and
ready code work should choose `no_action` rather than opening a PR. To validate a
real implementation path, provide a clean checkout and command:

```sh
export HARNESS_CODING_WORKSPACE_ROOT=/path/to/checkout
export HARNESS_CODING_WORKSPACE_COMMAND='your-coder --context "$HARNESS_CODING_WORKSPACE_CONTEXT"'
export HARNESS_CODING_WORKSPACE_REMOTE=origin
export HARNESS_CODING_WORKSPACE_PUSH=1
./run.sh start
```

The command receives a JSON context path in `HARNESS_CODING_WORKSPACE_CONTEXT`.
It must leave a meaningful non-`.harness*` product diff; the local-git provider
commits and pushes the branch, then the workflow opens the PR through `RoleTools`.
Use these focused checks before a full demo run:

```sh
cargo test -p harness-production coding_workspace_tests::local_git_workspace_accepts_product_code_or_docs_diff
HARNESS_FORGEJO_E2E=1 cargo test -p harness-testing --test forgejo_workspace_pr -- --ignored --test-threads=1
```

## What it does

Boots Forgejo + a host-mode `forgejo-runner`, starts the local webhook trigger,
provisions each configured repo with labels, CI, and a webhook, then seeds the
source repo with one cross-repo parent intake issue by default. It launches
exactly one `harness-worker` per role-with-an-agent plus one mechanical
reconciler. Workers still use wall-clock polling as the liveness backstop;
webhooks only wake them early.

With quarantined reference-delivery test fixtures (the gated `harness-testing`
e2e), the source intake issue then flows through the cross-repo workflow as
below. The production launcher uses generic manifest-driven agents, so steps that
require fixed child-issue fan-out or real code/PR-head creation must come from
workflow configuration, declared external tools, and runner bindings rather than
synthetic production behavior:

1. **architect** fans the parent intake out into one child `code` issue per
   configured repo and blocks the parent on those children;
2. **engineer** claims each child (`in-progress`) in its own repo, prepares a
   real head branch, and opens an implementation PR;
3. the **`forgejo-runner`** runs real CI on each PR head;
4. **reviewer** approves each PR;
5. **owner** merges once each PR's CI + review gates are green;
6. post-merge, test fixture adapters close produced code issues and clear their
   `in-progress` labels; production generic agents require that behavior to be
   modeled in workflow/user configuration or external tooling before dependency
   aggregation can unblock the parent.

The **mechanical** worker runs the controller plane (lease expiry, partial-
transition repair, dependency unblock) without an agent. See
[`docs/explanation/forgejo-e2e-topology.md`](../../docs/explanation/forgejo-e2e-topology.md)
for the durable topology and real-CI design.

## Two-repo walkthrough

With the checked-in config, `acme/service` is the source repo and
`acme/service-canary` is the second target. The source intake body names both
repo ids (`forgejo:acme/service` and `forgejo:acme/service-canary`) and asks the
architect for one child per repo. In Forgejo you should see:

1. one parent intake in `acme/service`;
2. one child code issue in `acme/service` and one in `acme/service-canary`;
3. one implementation PR per child, each in that child's repository;
4. the parent remaining blocked until both child PRs have merged and reconciled.

## Cross-repo production model

- Run **one worker per role**, not one worker pool per repository. The same role
  process scans every configured repo.
- Use **one token identity per role** with Forge access to every involved repo.
  Labels, CI workflow, and webhooks are ensured **per repo** during provisioning
  and worker startup.
- The source parent links to children with repo-qualified artifact references;
  child creation uses global correlation keys so re-scans do not duplicate work.
- Webhooks carry repo-specific hints that wake the shared pool and prioritize the
  hinted repo; polling remains the correctness backstop.
- Children land independently. The parent is an aggregation record, not an atomic
  cross-repo transaction.

## Watching progress and validating webhooks

Open the Forgejo UI at `BASE_URL` (log in as any provisioned role). In the
first configured repo, open the seeded parent intake. Watch the architect create
child code issues across the repo set, then watch each child repo's PR open, CI
run, review land, and merge. The parent should unblock only after all children
land. Worker logs land under `logs/` (created at run time). When webhooks are
enabled, the trigger wakes the fixed worker pool for events from any repo. Wake
payloads carry the repository hint; configured repos are scanned first and
unknown-repo hints are logged by workers and treated as a broad scan:

- `logs/provision.log` records `repo=owner/name cross_repo_parent_url=...` for
  the source repo, `repo=owner/name no_intake_seeded=cross-repo-target` for
  target repos, and `repo=owner/name webhook registered url=...` for each repo;
- `logs/trigger.log` reports `listening on`, `webhook accepted` or
  `webhook rejected`, and `wake_delivery outcome=no_sockets|sent|all_failed`
  with target/sent/failed counts;
- worker logs report `consumed authenticated wake` and then
  `completed tick trigger=wake actions=N`. `actions=0` means the worker woke,
  scanned fresh Forge state, and found no queue item.

In another terminal, run:

```sh
./run.sh validate-multi-repo
```

It verifies that every configured repo appears in provisioning, trigger, and
worker startup logs, checks that only the source repo received the parent intake,
then summarizes accepted webhook deliveries, wake batches sent, per-worker wake
consumption, wake-triggered ticks, and whether any wake-triggered tick made
workflow progress (`actions>0`). For a cheaper generic wake check, use
`./run.sh validate-webhooks`. For a long-poll smoke, start the demo with
`POLL_MS=120000 ./run.sh`, wait until workflow movement appears in Forgejo for
each repo, then run `./run.sh validate-multi-repo`; it should pass before any
two-minute poll backstop is needed.

## Validated smoke paths

The default, hermetic multi-repo topology smoke is the ignored filesystem process
rehearsal:

```sh
cargo test -p harness-testing --test multi_repo_multiprocess -- --ignored
```

The live Forgejo/webhook smoke is explicitly gated because it boots Forgejo and a
host-mode runner:

```sh
HARNESS_FORGEJO_E2E=1 \
  cargo test -p harness-testing --test forgejo_multi_repo_webhook -- --ignored --test-threads=1
```

That live test validates the repo-hinted wake path. The cross-repo fan-out
scenario is covered by the gated Forgejo multiprocess suite and the deterministic
multi-repo process suite. The shell demo's own validation path is
`./run.sh validate-multi-repo` after a run; it validates the operator logs rather
than reasserting Forge state.

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
- **`provision binary is stale or incompatible` or `worker binary is stale or
  incompatible`:** rerun without `HARNESS_SKIP_BUILD=1`, or rebuild the
  development binaries manually with `cargo build -p harness-production`.
  `HARNESS_SKIP_BUILD=1` assumes `target/debug` and any `HARNESS_*_BIN`
  overrides are already current.
- **Wake consumed with `actions=0`:** the wake path worked; that worker simply
  had no active queue item after re-reading Forge state. Workers batch queued
  wake datagrams before a tick, so a webhook burst should show one
  `consumed authenticated wake batch ...` line followed by one
  `completed tick trigger=wake actions=0` follow-up per worker, rather than a
  long train of stale no-op scans.
- **Forgejo remains CPU-heavy after the workflow is done:** first check whether
  worker logs are still processing wake batches. Persistent `actions=0` batches
  mean webhooks are only causing fresh scans; `git cat-file` processes are normal
  Forgejo helpers and are not by themselves proof of active work. Also remember
  that `ps %CPU` is a lifetime average; use `top`, `pidstat`, or two `/proc/PID/stat`
  samples to see whether CPU dropped after stopping workers. The demo caps
  `GOMAXPROCS` for Forgejo/runner; lower `HARNESS_FORGEJO_GOMAXPROCS` if you
  need a tighter local CPU ceiling.
- **`Forgejo already responds` on start:** an orphaned or separately started
  server is still bound to `BASE_URL`. Run `./run.sh stop`; if pid files were
  lost, clean up with the orphan commands below.

## Teardown

`./run.sh stop` (or a `SIGINT`/`SIGTERM` to the running script) kills every spawned
process and removes the throwaway data dirs. If a run is force-killed, clean up
orphans with `pkill -f forgejo` / `pkill -f harness-worker` and remove
the run/data dirs.

## Point it at your own Forgejo

Change `BASE_URL` to your instance, set `REPOS` (or a production `--repo-list`)
to the scan shard, supply one real per-role token with access to every listed
repo that may receive child work, and skip the bundled server/runner +
provisioning steps. Ensure labels, CI, and webhooks per repo before filing the
parent intake. Replace `config/ci.yml` with your real CI and pair the engineer
with a real coding agent. The demo includes the ADR 0009 **webhook triggering**
accelerator for the local topology. For a real Forgejo, register a hook on each
repo, expose `WEBHOOK_URL`
over HTTPS to the Forgejo server, and keep the worker wake sockets host-local.
This is the same swap-to-real path documented in
[`docs/how-to/run-forgejo-multiprocess-e2e.md`](../../docs/how-to/run-forgejo-multiprocess-e2e.md)
and [`docs/explanation/forgejo-e2e-topology.md`](../../docs/explanation/forgejo-e2e-topology.md).
