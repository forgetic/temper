# Run the local basic-delivery deployment

> **Legacy walkthrough.** This page describes the superseded per-role pool
> (`smith-delivery.target` with architect/engineer/mechanical/trigger units).
> The consolidated two-tier deployment — one Temper daemon (the sole Forgejo
> API writer, deployed from `temper/deploy/`) plus one `smith-worker.service`
> — is documented in [`deploy/README.md`](../../deploy/README.md), including
> the cutover steps from this pool.
>
> The installer this page describes no longer exists in the repo: today's
> `deploy/install.sh` installs the two-tier worker (`smith-worker` plus
> `anvil-agent` from the sibling anvil checkout), and the binaries named below
> (`smith-coding-agent`, `temper-worker`, …) are no longer built anywhere. The
> page is kept as a record of the still-deployed legacy fleet until the
> operator cutover completes.

This guide installs and runs the basic-delivery workflow against `ai/smith` as
**real systemd user services** — the legacy deployment counterpart to the
throwaway daemon/worker [`examples/basic-delivery`](../../examples/basic-delivery/README.md)
launcher. A filed intake issue is driven all the way to a merged PR with **no
human action**, and **CI is the only landing gate**.

It is the deployment half of issue #9 (the install script + units + config live
in [`deploy/`](../../deploy/README.md)); the identity/label/webhook provisioning
half is [Provision `ai/smith`](provision-smith-dogfood.md). Read this for
bring-up, validation, and teardown.

> Scope. This sets up the worker pool, config, and unit files on your machine and
> starts the services. It does **not** create the `ai/smith` repo, commit CI, or
> provision Forgejo identities — those are the maintainer's and the provisioner's
> jobs (see [provision-smith-dogfood.md](provision-smith-dogfood.md)).

## Topology

`smith-delivery.target` groups four user services; Forgejo + the host-mode runner
stay in the existing `forgejo.service`:

```text
forgejo.service              Forgejo server + host-mode runner (existing, separate)
smith-delivery.target
  ├─ smith-architect.service   role worker: triage thin intake → ready code spec
  ├─ smith-engineer.service    role worker: implement → open implementation PR
  ├─ smith-mechanical.service  bot: stamp untriaged + land CI-green PRs (sole authority)
  └─ smith-trigger.service     local webhook accelerator → worker wake sockets
```

Live config is `~/.config/smith/`; live state (per-role checkouts, wake sockets)
is `~/.local/state/smith/`. Only templates are tracked in the repo.

## Prerequisites

- The existing `~/.config/systemd/user/forgejo.service` running a local Forgejo
  (with the host-mode runner and Actions enabled) at the `BASE_URL` you will set
  in `smith.env` (default `http://127.0.0.1:3000`).
- A built Temper checkout, by default a sibling `../temper` of this repo,
  providing `temper-worker` and `temper-trigger-forgejo`. Override its location
  with `TEMPER_WORKSPACE_ROOT`.
- This Smith checkout (it builds `smith-coding-agent` and
  `smith-workflow-role-decision`).
- `ai/smith` already provisioned: the `architect` / `engineer` / `bot` identities
  with repo-scoped `write`, the basic-delivery labels, the wake webhook, Actions
  enabled, and `~/.config/smith/secrets/roles.env` written — all from
  [provision-smith-dogfood.md](provision-smith-dogfood.md).
- `.forgejo/workflows/ci.yml` on `main` (maintainer-owned), so the landing gate
  has real CI to evaluate.

## 1. Install the deployment assets

From the repo root:

```sh
deploy/install.sh
```

This idempotent installer:

- builds the four binaries and installs them into `~/.local/bin`
  (`temper-worker`, `temper-trigger-forgejo`, `smith-coding-agent`,
  `smith-workflow-role-decision`);
- copies the systemd user unit templates into `~/.config/systemd/user/`;
- copies the config templates into `~/.config/smith/` **without overwriting any
  file you have already edited** (`smith.env`, `workflow.json`,
  `prompts/{architect,engineer}.md`, `secrets/roles.env.example`);
- generates `~/.config/smith/secrets/{webhook-secret,wake-secret}` (mode `0600`)
  if absent;
- creates `~/.local/state/smith/{architect,engineer}` and the wake-socket dir.

Re-run it any time to pick up unit/template fixes — your edited config and your
secret files are preserved. Useful overrides:

| Variable | Effect |
| --- | --- |
| `TEMPER_WORKSPACE_ROOT=/path/to/temper` | where to build `temper-worker` / `temper-trigger-forgejo` |
| `SMITH_SKIP_BUILD=1` | skip `cargo build` (use already-current binaries) |

Config, state, and binaries are pinned under `~/.config/smith`, `~/.local/state/smith`,
and `~/.local/bin` to match the unit templates (which hardcode `%h/.config` and
`%h/.local`, like the existing `forgejo.service`).

## 2. Review `~/.config/smith/smith.env`

The installer drops a template; edit it for your instance. It is read as a
systemd `EnvironmentFile`, so plain `KEY=VALUE` only. Key knobs:

- `BASE_URL`, `OWNER`, `NAME` — the repo to drive (`http://127.0.0.1:3000`,
  `ai`, `smith`).
- `POLL_MS` — long role-worker poll (webhooks are the accelerator; polling is the
  backstop).
- `CI_STATUS_POLL_MS` / `IDLE_POLL_MAX_MS` — the mechanical worker's short CI
  poll and idle backoff (Forgejo 7.0.x does not webhook on Actions completion).
- `TRIGGER_BIND` — loopback bind for the webhook trigger; the repo's registered
  webhook URL must point at `http://<TRIGGER_BIND>/forgejo/webhook`.
- `SMITH_WORKFLOW_ROLE_DECISION_ARGS_JSON` / `SMITH_CODING_AGENT_ARGS` — Smith
  provider/auth options (never Forge tokens).

Optionally edit the coding-agent overlays in
`~/.config/smith/prompts/{architect,engineer}.md` — additive house-style guidance
the [coding agent honors](../reference/coding-agent-prompts.md). Delete a file to
run with just the built-in prompt.

## 3. Provision, clone workspaces, and set up auth

If you have not already, complete the provisioning runbook now — it writes
`~/.config/smith/secrets/roles.env`, pre-clones the per-role workspaces into
`~/.local/state/smith/{architect,engineer}/smith` (architect read-only; engineer
writable + push creds), and sets up Smith provider auth:

- [Provision `ai/smith`](provision-smith-dogfood.md) — steps 2, 4, and 5.

The units fail to start until `roles.env` exists, so do this before bring-up.

## 4. Bring the pool up

```sh
systemctl --user daemon-reload
systemctl --user enable --now smith-delivery.target
```

`enable --now` starts the four units (and re-starts them at next login). To run
them for this session only, use `start` instead of `enable --now`.

## 5. Validate

Confirm every unit is active and the wake path is live:

```sh
systemctl --user status smith-delivery.target
systemctl --user list-units 'smith-*'
journalctl --user -u smith-architect -u smith-engineer -u smith-mechanical -u smith-trigger -n 50
```

Look for, in journald:

- each worker reaching `completed tick trigger=initial` (startup scan done);
- `smith-mechanical` recording `ci_reader=bot` and **no**
  `no web-UI credentials configured for the CI read fallback` /
  `forgejo web-ui login failed` lines (it has the bot web-UI creds it needs);
- `smith-trigger` logging `listening on`, then `webhook accepted` and
  `wake_delivery outcome=sent` once activity flows;
- woken workers logging `consumed authenticated wake` and
  `completed tick trigger=wake actions=…`.

Validate the landing gate locally (the same checks CI runs) before driving a real
intake:

```sh
cargo fmt --all -- --check
cargo dev-clippy
cargo dev-check
cargo dev-test
```

## 6. Drive it

File one **unlabeled** intake issue as the `agent` user (the external filer) and
watch it progress with no further human action:

```text
unlabeled
  → untriaged           (bot: raw_intake mark_untriaged)
  → code + ready        (architect: rewrites the thin body into a real spec)
  → implementation PR   (engineer: real product diff)
  → CI green            (forgejo-runner runs .forgejo/workflows/ci.yml)
  → auto-merge + landed (bot: land_pr, gated on CI alone)
```

Follow it live:

```sh
journalctl --user -u smith-architect -u smith-engineer -u smith-mechanical -f
```

## Teardown

Stop the pool (Forgejo keeps running in its own unit):

```sh
systemctl --user disable --now smith-delivery.target
```

`disable --now` stops the units and unhooks them from login. Live config, state,
and secrets under `~/.config/smith` and `~/.local/state/smith` are left in place;
remove those directories by hand if you want a clean slate. To uninstall the unit
files, delete `~/.config/systemd/user/smith-*.service` and
`smith-delivery.target`, then `systemctl --user daemon-reload`.

## Troubleshooting

- **A unit fails immediately.** `journalctl --user -u smith-engineer -e`. The
  usual cause is a missing `~/.config/smith/secrets/roles.env` (run provisioning)
  or a missing binary in `~/.local/bin` (re-run `deploy/install.sh`).
- **PRs never land.** Check `smith-mechanical` journald for the ADR-0019 CI-read
  fallback errors above; the bot needs both its REST token and web-UI
  username/password in `roles.env`.
- **Workers only act on the slow poll.** The webhook path is not waking them:
  confirm `smith-trigger` is listening and the repo's registered webhook URL
  matches `TRIGGER_BIND`. Polling still makes progress, just slowly.

## References

- [`deploy/README.md`](../../deploy/README.md) — the tracked templates + installer.
- [Provision `ai/smith`](provision-smith-dogfood.md) — identities, labels,
  webhook, workspaces, provider auth.
- [`examples/basic-delivery/README.md`](../../examples/basic-delivery/README.md)
  — the topology, the `bot`/CI-read requirement, and acceptance/validation.
- [Configure provider auth](configure-provider-auth.md).
- [Coding-agent prompt overlays and `AGENTS.md`](../reference/coding-agent-prompts.md).
- [Workflow role observability](../reference/workflow-role-observability.md) —
  reading the trigger/role wake logs.
