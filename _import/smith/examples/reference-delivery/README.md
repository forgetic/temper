# Reference-delivery example

A **Smith-owned operator demo** of Temper's production daemon/worker topology. It
is the richer counterpart to [`examples/basic-delivery/`](../basic-delivery/): a
throwaway Forgejo server, a host-mode `forgejo-runner` producing real CI,
`temper-provision-forgejo`, **one `temper-daemon`**, and **one `smith-worker`**
serving the architect, engineer, and reviewer roles across the configured repo
set. The launcher defaults to a sibling Temper checkout at `../temper`; set
`TEMPER_WORKSPACE_ROOT=/path/to/temper` if your checkout layout differs.

The proof this example exists to show is the reference workflow's full trail:
thin human intake, architect triage/rewrite or cross-repo breakdown, engineer
implementation PR, reviewer native approval, CI-gated landing, and mechanical
bot merge.

## Honest framing — read this first

This is a demo, not a turnkey production deployment. It is honest about what is
real and what is canned:

- **Real:** Forgejo is a real server, the runner executes real Forgejo Actions CI,
  the provisioner creates real users/tokens/labels/hooks/workflow state, the
  daemon owns real Forge API mutation authority, and the worker runs real git
  workspaces with role credentials. Cross-repo mode writes real parent/child
  issue state and dependency refs into Forgejo.
- **Real:** the process shape is the production two-tier shape: one daemon for
  queue scanning, webhooks, leases, apply tokens, result application, and
  mechanical landing; one worker for coding-executor jobs across many
  `(repo, role)` capabilities.
- **Canned:** the seeded request is a greeting/banner stand-in. It is deliberately
  small so operators can see the workflow move without needing a project-specific
  backlog item.
- **Canned:** `config/ci.yml` is demo CI. It parse-checks shell scripts so a
  greeting diff can land through a genuine CI gate. Replace it with your real
  build/test workflow for a real project.
- **Optional canned agent:** by default the worker spawns the sibling anvil
  checkout's `anvil-agent`. Set `REFERENCE_DELIVERY_CODER=greeting` to use the
  deterministic `tools/greeting-coder.sh` stand-in for an offline/no-provider
  smoke. That stand-in still traverses the real daemon/worker and Forgejo state.

## Topology

`run.sh` boots every process locally from development-profile binaries and writes
per-process logs under `logs/`:

```text
                 Forgejo :4200 + host-mode forgejo-runner
                 real repos, issues, PRs, reviews, CI, merges
                         │
                         │ repo webhooks registered by provisioner
                         ▼
┌──────────────────────────────────────────────────────────────────┐
│ temper-daemon                                logs/daemon.log      │
│                                                                  │
│  POST /forgejo/webhook                                           │
│  webhook wake scans + long poll backstop                         │
│  short mechanical CI/landing backstop                            │
│  leases and queue assignment                                     │
│  per-role Forge API tokens for verdict/PR/review appliers        │
│  cross-repo child materialisation and dependency refs            │
└───────────────────────────────┬──────────────────────────────────┘
                                │ worker protocol
                                ▼
┌──────────────────────────────────────────────────────────────────┐
│ smith-worker --executor coding              logs/worker.log      │
│                                                                  │
│  one process, many --capability <repo>:<role> entries            │
│  persistent workspaces under run/workspaces/                     │
│  role git credentials for clone/fetch/commit/push                │
│  runs the configured --agent-command per assigned job            │
│  no Forge API token and no Forge API mutation path               │
└──────────────────────────────────────────────────────────────────┘
```

`temper-provision-forgejo` runs before the daemon starts to create each repo,
users, labels, CI, and webhook while holding intake back. After the daemon and
worker are ready, a seed-only provision pass files intake so the issue-created
webhook demonstrates the daemon wake-scan path.

## Roles

`config/workflow.json` declares the reference workflow roles `architect`,
`engineer`, `reviewer`, `owner`, `human`, and the mechanical automation role.
This launcher deliberately serves only:

- `architect` — triages intake with `triage_workspace`, rewrites the body on
  `ready_code`, or returns `needs_breakdown` children for cross-repo fan-out;
- `engineer` — implements ready code issues and produces PR heads;
- `reviewer` — reviews the real PR diff/CI and returns `approve`, `changes`, or
  `escalate`.

`owner` and `human` are **workflow-declared but unserved**: they are not passed
as daemon `--role` values and they are not included in worker `--capability`
entries. That is the safe-idle state. Items routed to those queues remain visible
in Forgejo but no local agent claims them. `human` still matters because the
workflow's `intake_author` is the provisioned human role; the seed-only pass
files the intake as that external filer rather than as an implementation role.
Mechanical landing is handled inside the daemon using the provisioned `bot`
automation identity, not by a Smith worker capability.

## The happy path

The single-repo default (`REPOS=` with `OWNER=acme`, `NAME=service`) converges
through this path:

1. `run.sh` provisions the repo but deliberately does **not** file intake yet.
2. `run.sh` starts one daemon and one worker. The worker registers capabilities
   for `acme/service:architect`, `acme/service:engineer`, and
   `acme/service:reviewer`.
3. `run.sh` files the thin human-authored intake **last**. The issue-created
   webhook reaches `POST /forgejo/webhook`, is accepted, and drives a daemon wake
   scan instead of waiting for the poll backstop.
4. The daemon's mechanical automation marks the raw intake `untriaged`, then
   assigns the architect a read-only triage job.
5. The architect's `triage_workspace` returns `verdict=ready_code` plus a
   rewritten body. The daemon applies `triage_intake_to_code`: the issue body is
   replaced with the architect's code spec and the issue receives `code` +
   `ready`.
6. The daemon assigns the engineer a writable coding job. The worker prepares a
   persistent workspace under `run/workspaces/`, runs the configured agent
   command, commits the product diff, and pushes a role-authored branch.
7. The daemon creates the implementation PR as the engineer. The workflow artifact
   definition attaches `implementation` as the identifying label and
   `needs-reviewer` from `implementation_pr.initial_labels` at creation.
8. `forgejo-runner` runs real CI on the PR head.
9. The daemon assigns the reviewer. The reviewer returns `approve`; the daemon
   submits a native approval review and adds `landing`.
10. The daemon's mechanical backstop sees a CI-green, approved landing PR and
    merges it as `bot`. Forgejo closes the source issue from the merge commit
    trailer.

## Cross-repo mode

Cross-repo mode is ported to the same daemon/worker shape. Set `REPOS` to two or
more repos, or force `CROSS_REPO_INTAKE=1`, to seed one parent intake in the
first configured repo. Its body is machine-written by `run.sh` and carries one
plan line per target repo with two stable markers:

- the exact `owner/name` value in `target_repo`;
- the stable child `slug` for that repo.

The architect reads those markers and returns `verdict=needs_breakdown` with
children over the worker protocol in `JobResult.children`. The daemon applies the
breakdown by materialising one child code issue per `target_repo`. It records
repo-qualified parent backrefs in child bodies, global child correlation keys to
avoid duplicates across rescans, and dependency refs on the parent. Each child
then runs the same engineer → reviewer → merge path in its own repo.

This mode converges offline with `REFERENCE_DELIVERY_CODER=greeting`: the
stand-in parses the machine-written plan, emits one child per repo, implements
the greeting diff in each child repo, approves the PRs, and lets the daemon land
them through real Forgejo state. That is the cross-repo convergence proof the old
fleet shape never provided.

## Binary prerequisites

`run.sh` keeps the demo entry point self-healing. Unless `TEMPER_SKIP_BUILD=1` is
set, start refreshes the development binaries before boot:

- in the Temper checkout, `cargo build -p temper` for
  `temper-daemon`, `temper-provision-forgejo`, and
  `temper-validate-reference-delivery`;
- in this Smith checkout, `cargo build -p smith-worker`;
- in the default anvil-agent mode, `cargo build --bin anvil-agent` in the
  sibling anvil checkout.

The launcher then refuses stale or incompatible binaries with `--help` probes:

- `temper-provision-forgejo` must advertise `--workflow`, `--seed-intake`, and
  `--seed-only`;
- `temper-daemon` must advertise `--mechanical-cadence-secs`;
- `smith-worker` must advertise `--executor`.

Other prerequisites:

- pinned Forgejo `7.0.12` and `forgejo-runner` `3.5.1`. Leave
  `TEMPER_FORGEJO_BINARY` / `TEMPER_FORGEJO_RUNNER_BINARY` empty to use the cache
  under the Temper checkout's `.cache/forgejo/`, or set explicit paths;
- a host that permits host-mode CI jobs. The runner executes steps directly on
  the host, no containers;
- Smith provider/auth for the default LLM mode. Smith owns provider setup;
  Temper and `smith-worker` only pass opaque agent arguments and role git
  credentials.

## Layout

```text
examples/reference-delivery/
├── README.md             # this file
├── observability.md      # daemon/worker evidence and validation guide
├── .gitignore            # ignores runtime run/, logs/, pid files, and secrets
├── config/
│   ├── temper.env        # operator-editable knobs (no secrets)
│   ├── workflow.json     # bundled reference workflow spec
│   ├── intake-issue.md   # deliberately thin single-repo intake body
│   └── ci.yml            # host-mode demo CI applied to provisioned repos
├── tools/
│   └── greeting-coder.sh # deterministic stand-in agent
├── secrets/              # gitignored except templates + .gitignore
│   └── .env.example
└── run.sh                # launcher / validation / teardown
```

`config/workflow.json` tracks Temper's canonical reference-delivery fixture.
Keep workflow semantics in sync with the Temper checkout when changing roles,
labels, artifact kinds, transitions, or gates.

## Quick start

From this directory, the basic lifecycle is two commands:

```sh
./run.sh start
./run.sh stop
```

`./run.sh start` blocks, prints the Forgejo URL, and tears everything down on
Ctrl-C. `BASE_URL` defaults to `http://127.0.0.1:4200`. `./run.sh stop` tears
down a previous run via saved PIDs and removes the throwaway Forgejo state.

Use the deterministic offline path when you want a provider-free smoke of the
full topology:

```sh
REFERENCE_DELIVERY_CODER=greeting ./run.sh start
```

The default `REFERENCE_DELIVERY_CODER=anvil` spawns the built `anvil-agent`
(via `--agent-command anvil-native`) with `ANVIL_AGENT_ARGS` (default
`--auth chatgpt-oauth`). The greeting mode binds `tools/greeting-coder.sh` as the
worker's `--agent-command`: architect triage/breakdown, engineer edits, and
reviewer approval are fixed and deterministic, while the daemon, worker, Forgejo,
CI, PRs, reviews, dependencies, and merges remain real.

## Knobs in `config/temper.env`

The config file contains the operator-editable knobs that `run.sh` reads. Any
matching non-empty environment variable exported before `./run.sh` wins over the
file; `REPOS=` is special and intentionally selects the `OWNER`/`NAME` fallback.
Secrets never live in `config/temper.env`; provisioning writes role identities to
`secrets/roles.env`, the webhook secret to `secrets/webhook-secret`, and
operator-only provider overrides may go in gitignored `secrets/.env`.

- **Repository and workflow:** `OWNER`, `NAME`, `REPOS`, `DEFAULT_BRANCH`, and
  `WORKFLOW_FILE`. Leave `REPOS` empty for one repo; set space-separated
  `owner/name` paths for multi-repo mode.
- **Served roles:** `SERVED_ROLES`. The default is `architect engineer reviewer`;
  do not add `owner` or `human` unless you also intend to serve those queues.
- **Seeded intake:** `INTAKE_TITLE`, `INTAKE_BODY_FILE`,
  `CROSS_REPO_INTAKE`, and `CROSS_REPO_INTAKE_TITLE`. `CROSS_REPO_INTAKE=auto`
  seeds one cross-repo parent when `REPOS` has at least two entries.
- **Endpoints and daemon cadence:** `BASE_URL`, `DAEMON_BIND`, `WEBHOOK_URL`,
  `DAEMON_POLL_CADENCE_SECS`, `DAEMON_MECHANICAL_CADENCE_SECS`,
  `DAEMON_LEASE_TTL_SECS`, and `RUN_SECS`. The long poll cadence is the liveness
  backstop; webhooks are the fast path. The short mechanical cadence handles CI
  reads and landing.
- **Bundled Forgejo runtime:** `TEMPER_FORGEJO_GOMAXPROCS`,
  `TEMPER_FORGEJO_BINARY`, and `TEMPER_FORGEJO_RUNNER_BINARY`.
- **Temper entry points:** `TEMPER_DAEMON_BIN`, `TEMPER_PROVISION_BIN`,
  `TEMPER_VALIDATE_BIN`, and `TEMPER_BUILD_PACKAGE`.
- **Smith worker and anvil agent:** `SMITH_WORKSPACE_ROOT`, `SMITH_WORKER_BIN`,
  `ANVIL_WORKSPACE_ROOT`, `WORKER_MAX_CONCURRENT`, `REFERENCE_DELIVERY_CODER`,
  `ANVIL_AGENT_BIN`, and `ANVIL_AGENT_ARGS`.

The coding-agent binding is now just the worker's `--agent-command`. `run.sh`
starts `smith-worker --executor coding` with a workspace root under
`run/workspaces/`, one `--capability <repo>:<role>` per configured pair, role git
credentials from `secrets/roles.env`, and either the anvil-native surface
(spawning the built `anvil-agent`) or `tools/greeting-coder.sh` as that
command. The worker owns checkout
preparation, read-only vs writable job handling, commits, pushes, and result
submission.

## Validation

Run validation before `./run.sh stop`; the Forgejo server is throwaway and
teardown removes the live state.

```sh
./run.sh validate-webhooks
```

This reads `logs/provision.log`, `logs/daemon.log`, and `logs/worker.log`. The
expected daemon evidence chain is: webhook registered → webhook accepted →
webhook wake scan → work enqueued by that scan → job assigned → result received.
The expected worker evidence chain is: worker registered → assignment accepted →
result sent. The validator also checks that the daemon's mechanical landing path
has usable bot credentials and reported no CI-read credential failure.

```sh
./run.sh validate-multi-repo
```

This includes the webhook checks, then requires per-repo provisioning,
assignment, and worker evidence. When cross-repo intake is enabled it also runs
Temper's retained `temper-validate-reference-delivery` Forge-state validator. The
Forge-state check requires one child per configured repo, child bodies with
repo-qualified parent backrefs and correlation metadata, and parent dependency
refs back to those children.

## Watching progress

See [`observability.md`](observability.md) for the exact daemon and worker log
lines to inspect while a run is active. In the Forgejo UI, log in as any
provisioned role and watch the seeded issue or cross-repo parent move through the
labels and PR/review/merge state described above. `logs/provision.log` records
seeded issue URLs (`intake_issue_url=` or `cross_repo_parent_url=`) for quick
navigation.

## Validated smoke paths

This shell example is operator-driven: start it, watch Forgejo/logs, then run the
validators before teardown. Its closest automated relatives are:

- Temper's daemon Forgejo e2e test, `tests/daemon_forgejo_e2e.rs`; see
  `../temper/docs/how-to/run-daemon-e2e.md` in the sibling Temper checkout;
- Smith's hermetic out-of-process worker e2e in
  `crates/smith-worker/tests/coding_worker_e2e.rs`.

## Troubleshooting

- **Webhook registered but not accepted:** confirm `WEBHOOK_URL` points at the
  daemon's reachable `POST /forgejo/webhook` route, the webhook secret file
  exists, and `logs/daemon.log` shows `temper-daemon: serving on ...` before
  intake is seeded. Also confirm the bundled Forgejo config allows loopback
  webhooks.
- **Webhook accepted but no assignment:** compare daemon `--repo`/`--role` values
  with worker `--capability` entries. The worker log should contain
  `smith-worker: registered worker_id=... capabilities=N`; if it does not, the
  worker failed before registration.
- **Run converges slowly:** webhooks are the fast path; `DAEMON_POLL_CADENCE_SECS`
  is only the correctness backstop. A missed webhook should recover at the next
  poll scan, while mechanical CI/landing work follows
  `DAEMON_MECHANICAL_CADENCE_SECS`.
- **Stale binary error:** rerun without `TEMPER_SKIP_BUILD=1`, or manually rebuild
  the relevant checkout. The launcher's `--help` probes name the missing flag
  (`--workflow`/`--seed-intake`/`--seed-only`, `--mechanical-cadence-secs`, or
  `--executor`).
- **Forgejo already responds on start:** an old server is still bound to
  `BASE_URL`. Run `./run.sh stop`; if pid files were lost, clean up orphans with
  `pkill -f forgejo`, `pkill -f forgejo-runner`, `pkill -f temper-daemon`, and
  `pkill -f smith-worker`.
- **Validation fails after teardown:** rerun the demo and validate before stopping
  it. `stop` removes the throwaway Forgejo database and runtime workspaces while
  keeping logs for inspection.

## Point it at your own Forgejo

Set `BASE_URL` to your instance, provide real role credentials, ensure each repo
has the workflow labels/CI/webhook, and skip the bundled server/runner bootstrap
for a durable deployment. Keep the same split: Temper's daemon tier owns Forge
API mutation and mechanical landing; Smith's worker tier owns role git
workspaces and agent execution. Replace `config/ci.yml` with project CI and pair
the engineer with an agent whose diffs pass that gate.
