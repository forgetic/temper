# Provision `ai/smith` and bring up the basic-delivery dogfood

This runbook brings the no-human, agent-driven
[basic-delivery example](../../examples/basic-delivery/README.md) up against a
real `ai/smith` repo: a filed intake issue is driven all the way to a merged PR
with **no human action**, and **CI is the only landing gate**.

It is the operator counterpart to that example's README — the example explains
*what* the topology demonstrates; this guide is the *do-this-in-order* checklist
for a production-like local deployment that points at an existing `ai/smith`.

> Scope. This provisions identities, labels, the wake webhook, and Actions on an
> **already-created** `ai/smith` repo; it does **not** create the repo, commit
> CI, or grant the `ai` org new ownership. CI on `main` is owned by the
> maintainer (step 3). Provider/auth setup is owned by Smith (step 5).

## Prerequisites

- An existing `ai/smith` repo on the target Forgejo instance, reachable at
  `--base-url` (e.g. `http://127.0.0.1:3000`).
- The `agent` user has Forgejo **admin** on that instance (used only to mint a
  short-lived provisioning token — see step 1).
- A built Temper checkout providing `temper-provision-forgejo`,
  `temper-worker`, and `temper-trigger-forgejo`, including the
  `--existing-repo` / `--access repo-collaborator` provisioning support tracked
  in `../temper/provision-tweaks.md`. The basic-delivery launcher refreshes
  `target/debug` on start unless `TEMPER_SKIP_BUILD=1`.
- The sibling anvil checkout for the LLM roles: the basic-delivery launcher
  auto-builds `anvil-agent` from `../anvil` and binds it via
  `--agent-command anvil-native`.
- The pinned `forgejo` / `forgejo-runner` binaries staged (see the example's
  Prerequisites), and a host that permits **host-mode** CI jobs.

## Steps

### 1. Mint an admin token (provisioning only)

The `agent` user has Forgejo admin. Mint a token used **only** for provisioning
and pass it via the environment — **never on argv**:

```sh
export TEMPER_FORGEJO_ADMIN_TOKEN=…   # a fresh agent-admin token, kept out of shell history
```

Revoke it once provisioning is done; the running workers use the role-scoped
tokens written to `roles.env` (step 2), not this admin token.

### 2. Provision identities, labels, and the wake webhook

Run the provisioner against the **existing** repo. This creates the
`architect` / `engineer` / `bot` identities with repo-scoped `write`, the six
basic-delivery labels, the wake webhook, enables Actions, and writes
`roles.env` — **without** creating the repo, committing any CI, or granting `ai`
org ownership:

```sh
TEMPER_FORGEJO_ADMIN_TOKEN=… temper-provision-forgejo \
  --base-url http://127.0.0.1:3000 --owner ai --name smith \
  --existing-repo --access repo-collaborator \
  --workflow ~/.config/smith/workflow.json \
  --webhook-url http://127.0.0.1:<trigger-port>/forgejo/webhook \
  --webhook-secret-file ~/.config/smith/secrets/webhook-secret \
  --seed-intake no \
  --out ~/.config/smith/secrets/roles.env
```

`--seed-intake no` is deliberate: the intake issue is filed **last** (step 7),
after the worker pool and wake trigger are listening, so the issue-created
webhook is what wakes the workers rather than a long poll.

### 3. CI on `main` (maintainer-owned)

The landing gate evaluates real CI, so `.forgejo/workflows/ci.yml` must be on
`main`. It is already present in this repo and runs on `pull_request` against
`main` (fmt + clippy + check + test via the `cargo dev-*` aliases). The
provisioner does **not** commit CI; keep it maintained on `main` through normal
PRs (gated by that same workflow). Verify it locally with the commands under
[Validate the gate](#validate-the-gate) before relying on it.

### 4. Pre-clone per-role workspaces

Clone the repo once per LLM role so workers do not race on first checkout:

```sh
~/.local/state/smith/architect/smith   # read-only checkout for the architect
~/.local/state/smith/engineer/smith    # writable checkout + push creds for the engineer
```

The architect's checkout is read-only; the engineer's is writable and carries
push credentials. This may be folded into the install script (issue #9 child B).

### 5. Provider auth (anvil-owned)

Log in once so `~/.pi/agent/auth.json` has the codex credential, then run
anvil's preflight (from `../anvil`):

```sh
pi /login openai-codex
cargo run --bin anvil -- preflight --auth chatgpt-oauth
```

See [Configure provider auth](configure-provider-auth.md) for the other
providers and override env vars. Never pass provider secrets on argv, and never
allow-list Forge tokens into the agent.

### 6. Start the delivery target

```sh
systemctl --user start smith-delivery.target
```

Confirm the worker pool and trigger came up via journald and the role/trigger
wake logs (see the observability reference below). The deployment units,
`~/.config/smith` config, and install script are tracked in issue #12.

### 7. Drive it

File one **unlabeled** intake issue as `agent` (mimicking an external filer) and
watch it progress with no further human action:

```text
unlabeled
  → untriaged              (bot: raw_intake mark_untriaged)
  → code + ready           (architect: rewrites the thin body into a real spec)
  → implementation PR      (engineer: real product diff)
  → CI green               (forgejo-runner runs .forgejo/workflows/ci.yml)
  → auto-merge + landed    (bot: landing land_pr, gated on CI alone)
```

## Validate the gate

CI is the only landing gate, so confirm it is green before driving a real
intake. The same checks the workflow runs:

```sh
cargo fmt --all -- --check
cargo dev-clippy
cargo dev-check
cargo dev-test
```

## Acceptance

A filed intake issue is driven to a merged PR with **no human action**, and **CI
is the only gate**. Confirm the proof the basic-delivery example exists to
provide: open the triaged issue and verify the **architect replaced the thin
seed body with a complete spec** (named interface, behavior, defaults, files to
touch, acceptance criteria) before the engineer implemented it.

## References

- [`examples/basic-delivery/README.md`](../../examples/basic-delivery/README.md)
  — the topology, the `bot`/CI-read requirement, and acceptance/validation.
- [Configure provider auth](configure-provider-auth.md) — step 5.
- [Workflow role observability](../reference/workflow-role-observability.md) —
  reading the trigger/role wake logs in steps 6–7.
- `../temper/provision-tweaks.md` — the Temper-side `--existing-repo` /
  `--access repo-collaborator` dependency for step 2.
