# Basic-delivery example

A **deliberately minimal**, no-human-in-the-loop operator demo: it drives a
single, thin intake issue **from submission to a merged PR with nobody in the
loop**. It is the "happy path, nothing fancy" counterpart to
[`examples/reference-delivery/`](../reference-delivery/): **one** repo, the
human-capable workflow roles `architect` and `engineer`, the `bot` mechanical
automation authority, CI, webhooks on from the start, and landing gated on **CI
alone** — no reviewer, no owner, no human.

Like reference-delivery it boots the production topology from development-profile
binaries, but in the consolidated two-tier shape: a throwaway Forgejo server, a
host-mode `forgejo-runner` producing real CI, `temper-provision-forgejo`, **one
`temper-daemon`**, and **one `smith-worker`** running the coding executor for the
architect and engineer roles. The launcher defaults to a sibling Temper checkout
at `../temper`; set `TEMPER_WORKSPACE_ROOT=/path/to/temper` if your checkout
layout differs.

The proof this example exists to show is the thin-intake → architect-spec-rewrite
step: the filed issue says only what the filer wants, and the architect must turn
that into an implementable code spec before the engineer can write code.

## What it demonstrates

The run converges through the daemon/worker topology with only one daemon and one
worker process:

1. `run.sh` boots throwaway Forgejo + runner, then runs
   `temper-provision-forgejo` against `config/workflow.json` with
   `--seed-intake no`. That first pass creates exactly **one org + repo**
   (`acme/service` by default), users/tokens, labels, CI, and the webhook, but it
   deliberately does **not** file the intake issue yet.
2. `run.sh` starts **one `temper-daemon`**. The daemon owns the Forgejo webhook
   route (`POST /forgejo/webhook`), long poll backstop, short mechanical CI/merge
   backstop, leases, per-role apply tokens, and result appliers.
3. `run.sh` starts **one `smith-worker`** with `--executor coding`, capabilities
   for `acme/service:architect` and `acme/service:engineer`, persistent
   workspaces, and per-role git credentials.
4. Only after the daemon and worker are ready, `run.sh` runs a second
   `temper-provision-forgejo --seed-only` pass to file **one unlabeled intake
   issue**. The issue is authored by the **site admin** because the workflow
   declares `intake_author: { "kind": "site_admin" }`; that matters because the
   seed-only pass is meant to mimic an external filer, not a workflow role.
5. Filing the issue last is the seed-last webhook-wake proof: the issue-created
   webhook reaches the daemon's `POST /forgejo/webhook` route, is accepted, and
   triggers a targeted wake scan instead of waiting for the long poll backstop.
6. The daemon's mechanical automation first stamps the raw intake as
   `untriaged`, then the daemon assigns the architect a **triage verdict job**
   under a lease. The checkout is read-only. The action vocabulary travels in
   `JobContext.allowed_verdicts` and is written by the worker into the workspace
   context file.
7. The worker runs the configured agent command. For the architect job it returns
   a success result carrying `verdict=ready_code` and the rewritten issue body.
8. The daemon applies the `triage_intake_to_code` transition as the architect
   identity under the lease: `set_body` replaces the thin seed with the
   architect's complete spec, and the issue receives the `code` and `ready`
   labels.
9. The daemon assigns the engineer a writable coding job. The same worker runs
   the agent in the engineer's persistent workspace, commits the product diff
   with a `Closes #<n>` trailer, and pushes the branch as the engineer.
10. The daemon ensures the implementation PR as the engineer. The workflow's
    `implementation_pr` identifying labels attach natively, so the PR enters the
    landing queue without an environment-label workaround.
11. `forgejo-runner` runs real CI on the PR head and it goes green.
12. The daemon's mechanical backstop sees the CI-green implementation PR,
    applies `land_pr` as the bot, auto-merges it, and Forgejo closes the source
    issue from the merge commit trailer.

No reviewer approves; no owner or human acts. The bot is the **sole landing
authority** and lands purely on CI.

## What this topology removes

Compared with the legacy fleet, this example now removes:

- no separate webhook trigger process;
- no separate local socket fan-out between a trigger and role workers;
- no per-role worker process pool;
- no role-decision responder process — the daemon scans queues and assigns
  concrete jobs;
- no PR-labels environment workaround — implementation PR labels come from the
  workflow artifact definition.

That is the same deployment shape used by the production templates: Smith's
`deploy/` for the worker tier and Temper's `../temper/deploy/` for the daemon.

## Binary prerequisites

`run.sh` keeps the demo entry point self-healing by rebuilding the development
binaries before start unless `TEMPER_SKIP_BUILD=1` is set. It then refuses stale
or incompatible binaries with `--help` probes:

- `temper-provision-forgejo` must advertise `--workflow`, `--seed-intake`, and
  `--seed-only`;
- `temper-daemon` must advertise `--mechanical-cadence-secs`;
- `smith-worker` must advertise `--executor`.

By default the Temper binaries come from the sibling Temper checkout's
`target/debug/` and the worker comes from this Smith checkout's `target/debug/`.
Override them with `TEMPER_PROVISION_BIN`, `TEMPER_DAEMON_BIN`, and
`SMITH_WORKER_BIN` when using prebuilt or release artifacts. `TEMPER_SKIP_BUILD=1`
only skips the refresh build; the stale-binary probes still run.

The `intake_author: site_admin` workflow field is part of the current runtime
contract: the first provision pass creates the repo without intake, and the later
seed-only pass files the issue as the site admin so the issue lands unlabeled and
its creation webhook drives the first visible progress.

Other prerequisites:

- The pinned Forgejo `7.0.12` and `forgejo-runner` `3.5.1` binaries. Leave
  `TEMPER_FORGEJO_BINARY` / `TEMPER_FORGEJO_RUNNER_BINARY` empty to use the
  pinned cache under the Temper checkout's `.cache/forgejo/`, or set them to
  explicit paths.
- A host that permits **host-mode** CI jobs (spawning child processes and binding
  loopback ports). The runner executes steps directly on the host, no containers.
- Anvil provider/auth for the default LLM mode. By default the launcher builds
  the sibling anvil checkout's `anvil-agent` and runs it with
  `--auth chatgpt-oauth`. Anvil owns provider/auth setup; Temper and
  `smith-worker` only pass opaque agent arguments and role credentials.

## Single-outcome triage (how the architect is constrained)

The architect's `triage_intake` declares a **single** outcome (`ready_code`). For
the run to converge deterministically the architect must emit exactly that
verdict — never a design or breakdown alternative.

A single routing outcome does **not** make the architect a rubber stamp. The
intake is seeded intentionally thin (intent only; see `config/intake-issue.md`),
and the architect's role guidance instructs it to **design**: expand that intent
into a complete spec and return it as the rewritten body. The daemon then applies
that body through the `triage_intake_to_code` `set_body` effect. Comparing the
seeded body to the triaged issue shows the architect's design contribution — that
is the proof this example exists to provide.

The verdict vocabulary now travels daemon → worker in the wire protocol:
`JobContext.allowed_verdicts` is a v1 additive field, and `smith-worker` writes it
into the workspace context file. `anvil-agent` reads that field and
constrains the role prompt to exactly that set, so a single-outcome triage
collapses to one valid verdict. The worker also enforces read-only verdict jobs:
a verdict must be present and in vocabulary, and the job is not allowed to commit
or push.

## Layout

```text
examples/basic-delivery/
├── README.md            # this file
├── .gitignore           # ignores runtime run/, logs/, pid files, and logs
├── config/
│   ├── temper.env       # operator-editable knobs (no secrets)
│   ├── workflow.json    # the 3-role basic-delivery spec (keep in sync with
│   │                    #   Temper's crates/temper-workflow/fixtures/
│   │                    #   basic-delivery.json)
│   ├── intake-issue.md  # the deliberately THIN intake body (intent only)
│   └── ci.yml           # host-mode CI applied over the provisioned marker CI
├── tools/
│   └── greeting-coder.sh  # deterministic architect/engineer stand-in
├── secrets/             # gitignored except templates + .gitignore
│   └── .env.example
└── run.sh               # launcher / validation / teardown
```

`config/temper.env` carries the knobs `run.sh` actually reads:

- repo and workflow: `OWNER`, `NAME`, `DEFAULT_BRANCH`, `WORKFLOW_FILE`;
- seed issue: `INTAKE_TITLE`, `INTAKE_BODY_FILE`;
- endpoints and daemon cadence: `BASE_URL`, `DAEMON_BIND`, `WEBHOOK_URL`,
  `DAEMON_POLL_CADENCE_SECS`, `DAEMON_MECHANICAL_CADENCE_SECS`,
  `DAEMON_LEASE_TTL_SECS`, `RUN_SECS`;
- local Forgejo runtime: `TEMPER_FORGEJO_GOMAXPROCS`, `TEMPER_FORGEJO_BINARY`,
  `TEMPER_FORGEJO_RUNNER_BINARY`;
- Temper entry points: `TEMPER_DAEMON_BIN`, `TEMPER_PROVISION_BIN`,
  `TEMPER_BUILD_PACKAGE`;
- Smith worker and anvil agent: `SMITH_WORKSPACE_ROOT`, `SMITH_WORKER_BIN`,
  `ANVIL_WORKSPACE_ROOT`, `WORKER_MAX_CONCURRENT`, `BASIC_DELIVERY_CODER`,
  `ANVIL_AGENT_BIN`, `ANVIL_AGENT_ARGS`.

Secrets never live in `config/temper.env`. Provisioning writes role identities to
`secrets/roles.env`; operator-only provider or local overrides may go in the
gitignored `secrets/.env`.

> **Keeping the spec in sync.** `config/workflow.json` is the example's copy of
> the basic-delivery spec. A canonical Temper fixture lives at
> `crates/temper-workflow/fixtures/basic-delivery.json` in the Temper checkout,
> with validation/route coverage in `crates/temper-workflow/tests/`. Update the
> two together when changing workflow semantics.

## Quick start

From this directory:

```sh
./run.sh                      # boot everything; Ctrl-C tears down
./run.sh validate-webhooks    # summarize webhook, daemon, and worker evidence
./run.sh stop                 # tear down a previous run via saved PIDs
./run.sh help                 # usage
```

Progress is printed without secrets (server URL, daemon URL, the seeded issue
URL, and where logs live); per-process logs land under `logs/`. The checked-in
long `DAEMON_POLL_CADENCE_SECS=120` is intentional: polling is only the liveness
backstop, while the seed-last webhook-wake path should make the demo visibly
progress before that two-minute deadline. Edit `config/temper.env` for the
org/repo, endpoint, cadences, binary paths, worker knobs, and anvil agent args;
any matching environment variable exported before `./run.sh` wins over the file.

### Offline / no-LLM smoke

The default `./run.sh` path uses the real `anvil-agent` binary. For a
fully provider-free smoke of the full daemon/worker head path, select the
deterministic stand-in:

```sh
BASIC_DELIVERY_CODER=greeting ./run.sh
```

There is no separate role-decision step in this topology: the daemon scans queues
and assigns concrete jobs. `BASIC_DELIVERY_CODER=greeting` binds
`tools/greeting-coder.sh` as the worker's agent command. Its no-push contract is:
for the read-only architect job it emits the `ready_code` verdict plus a complete
rewritten body; for the writable engineer job it writes the product diff
(`src/banner.sh`) and a `{"summary": ...}` result. The **worker** owns git
commit, push, and result delivery.

This greeting mode is only for the basic-delivery smoke/demo path. It is not a
replacement for the default anvil LLM agent when you want real design and coding
behavior.

## Coding agent binding

`smith-worker` is the coding-agent boundary. `run.sh` starts it with
`--executor coding`, a workspace root under `run/workspaces/`, the Forgejo base
URL, one `--capability <repo>:<role>` per role, and an `--agent-command`:

- default `BASIC_DELIVERY_CODER=anvil`: `--agent-command anvil-native`, pointing
  `--agent-program` at the `anvil-agent` binary built from the sibling anvil
  checkout, plus `ANVIL_AGENT_ARGS` (default `--auth chatgpt-oauth`);
- `BASIC_DELIVERY_CODER=greeting`: the deterministic `tools/greeting-coder.sh`
  stand-in.

The worker prepares a persistent workspace per `(repo, role)` under
`run/workspaces/` and spawns the agent command once per assigned job. At that
spawn boundary the file contract is the same for the real agent and the stand-in:
`TEMPER_CODING_WORKSPACE_CONTEXT` points to the JSON context file, and
`TEMPER_CODING_WORKSPACE_RESULT` points to the JSON result file the command must
write.

Role git identities come from `secrets/roles.env` as environment variables named
`TEMPER_FORGEJO_{USER,TOKEN,EMAIL}_<ROLE>`. They are exported to the worker
environment, not placed on argv. The worker owns checkout preparation, the
read-only vs writable capability, commit, push, and PR-head delivery; there is no
shared one-off clone or separate root/command binding for the launcher to manage.

## CI

`config/ci.yml` is a host-mode workflow. `run.sh` commits it over the provisioned
commit-message-marker CI before the daemon and worker start, so a real coder's
ordinary-message PR head clears the landing CI gate. It checks out the PR head
and parse-checks every shell script in the tree (`sh -n`); when the engineer
leaves a non-shell diff there is nothing to validate and the job passes through.
A real project replaces this file with its real CI (build, test, lint) and pairs
the engineer with a coder whose diffs pass it.

## Acceptance / validation

A converged run proves this sequence happened: the seeded issue body was rewritten
by the architect, the issue was stamped `code` + `ready` during triage, one
labeled implementation PR was created, real CI went green, the daemon's bot
mechanical backstop merged the PR, and Forgejo closed the source issue from the
merge commit trailer. Open the triaged issue and compare it with
`config/intake-issue.md`; the before/after is the proof that the architect did
real design work rather than just relabeling.

`./run.sh validate-webhooks` reads `logs/provision.log`, `logs/daemon.log`, and
`logs/worker.log`. The important daemon evidence is: webhook registered, webhook
accepted, webhook wake scan, work enqueued by that scan, job assigned, and result
received. The important worker evidence is: worker registered, assignment
accepted, and result sent. It also checks that the daemon mechanical CI-read path
has the bot credentials it needs and reported no missing/unusable web-UI
credentials.

`./run.sh stop` / Ctrl-C tears everything down cleanly; re-runs start fresh.

## Point it at your own Forgejo

Set `BASE_URL` to your instance and provide tokens, then replace the bundled
server/runner bootstrap with your real Forgejo and runner. Keep the same workflow
spec, provisioner flags, daemon/worker topology, CI-only landing gate, and
single-outcome triage. For a durable deployment rather than a throwaway launcher,
use Smith's `deploy/` assets together with Temper's daemon deployment assets.
