# `deploy/` — the Smith worker tier of the two-tier topology

Repo-tracked **templates** plus an idempotent **install script** for the
consolidated daemon/worker deployment. One long-lived `smith-worker` process
replaces the old per-role pool (`smith-engineer` / `smith-mechanical` /
`smith-trigger` + `smith-delivery.target`): the **Temper daemon** (deployed from
the sibling `temper/deploy/`) is now the sole Forgejo **API** writer — it owns
webhook intake, the poll backstop, queue scanning, leases, mechanical landing,
and all role-attributed Forge mutations — while this worker tier registers its
`(repo, role)` capabilities, long-polls the daemon for work, runs the coding
agent in persistent per-`(repo, role)` workspaces, pushes branches **as the
role** over git, and returns structured results. The worker **never calls the
Forgejo API**; the git plane is the deliberate exception.

Live config and state live outside the repo (`~/.config/smith`,
`~/.local/state/smith`); only the templates are tracked here.

## Layout

```text
deploy/
├── install.sh                     idempotent installer (builds smith bins, copies templates)
├── bin/                           ExecStart shim → ~/.local/bin/
│   └── smith-worker-launcher          turns smith.env knobs into smith-worker argv (no secrets)
├── systemd/                       user unit template → ~/.config/systemd/user/
│   └── smith-worker.service           the consolidated worker tier (one unit)
└── config/                        config templates → ~/.config/smith/
    ├── smith.env                      operator knobs (NO secrets), systemd EnvironmentFile
    ├── workflow.json                  the basic-delivery workflow spec
    ├── prompts/                       optional coding-agent overlays
    │   ├── architect.md
    │   └── engineer.md
    └── secrets/                       only template + .gitignore tracked
        ├── .gitignore
        ├── README.md
        └── roles.env.example          shape provisioning's roles.env must have
```

## What the unit does

`smith-worker.service` runs one `smith-worker` process via the
`smith-worker-launcher` shim:

- **registers capabilities** — the space-separated `owner/name:role` pairs in
  `WORKER_CAPABILITIES` (`smith.env`), e.g.
  `ai/smith:engineer ai/temper:engineer`;
- **long-polls the daemon** (`WORKER_DAEMON_URL`) for assigned work over the
  versioned `temper-worker-protocol` wire contract — pull, not push; the Forge
  lease held by the daemon is the real arbiter;
- **runs coding jobs** with `--executor coding`: each enriched job lands in a
  persistent per-`(repo, role)` git workspace under `WORKER_WORKSPACE_ROOT`
  (default `~/.local/state/smith/worker`; fetch + `checkout -B`, never wiped),
  spawns `anvil-agent`, requires a diff, commits and **pushes the branch
  as the role** using the git credentials from `secrets/roles.env`
  (`TEMPER_FORGEJO_{USER,TOKEN}_<ROLEKEY>`, optional
  `TEMPER_FORGEJO_EMAIL_<ROLEKEY>`), and reports a structured result;
- **never calls the Forgejo API** — PR creation, labels, comments, merges, and
  leases are the daemon's job, performed with the daemon's per-role API tokens.

Secrets never appear on argv: the unit loads `smith.env` (knobs) and
`secrets/roles.env` (git credentials) as systemd `EnvironmentFile=`s, and
`smith-worker` reads the suffixed role variables directly from its environment.

## Credential split (who holds what)

| Tier | Holds | Used for |
| --- | --- | --- |
| Temper daemon (`temper/deploy/`) | Forge **API** tokens (per role + bot) | webhook intake, scans, leases, PR create/update, mechanical landing |
| Smith worker (this `deploy/`) | per-role **git** credentials (`roles.env`) | clone/fetch + role-authored commit/push in persistent workspaces |

**No new secrets**: the worker reuses the provisioner-written
`~/.config/smith/secrets/roles.env` exactly as provisioned. The webhook secret
belongs to the daemon deployment (`~/.config/temper/secrets/`), not here.

## Install

```sh
deploy/install.sh
```

Builds the worker tier binaries (`smith-worker`, plus `anvil-agent` from the
sibling anvil checkout) into
`~/.local/bin` (skip rebuilds with `SMITH_SKIP_BUILD=1`), installs the launcher
shim and the `smith-worker.service` template, copies config templates into
`~/.config/smith/` **without clobbering existing live edits**, and creates the
workspace parent `~/.local/state/smith/worker`. It does **not** build any Temper
binaries (deploy the daemon tier from `temper/deploy/install.sh`), does not
provision Forgejo, generates no secrets, and starts nothing.

Bring-up after reviewing `~/.config/smith/smith.env`:

```sh
systemctl --user daemon-reload && systemctl --user start smith-worker.service
journalctl --user -u smith-worker.service -f
```

## Cutover from the legacy per-role pool

The old three-process pool and the new tier must not race each other on the
same repos. To switch a live host:

1. **Stop and disable the legacy units** (including the architect unit if it is
   installed):

   ```sh
   systemctl --user disable --now smith-delivery.target \
       smith-engineer.service smith-mechanical.service smith-trigger.service
   systemctl --user disable --now smith-architect.service 2>/dev/null || true
   ```

2. **Deploy and start the Temper daemon** from the sibling checkout:
   `temper/deploy/install.sh`, then configure `~/.config/temper/daemon.env`
   (repos/roles, mechanical cadence, webhook secret file, per-role API tokens)
   and start `temper-daemon.service`. The daemon subsumes the mechanical
   worker's landing/stamping and the webhook trigger.

3. **Re-point the repo webhooks**: each watched repo's webhook previously
   targeted the local trigger (`http://127.0.0.1:38090/...`); update it to the
   daemon's webhook route (`http://<daemon-bind>/forgejo/webhook`, default bind
   `127.0.0.1:8080`), keeping the same HMAC secret the daemon is configured
   with. The daemon's poll backstop covers any missed deliveries meanwhile.

4. **Start the worker tier**: `systemctl --user daemon-reload && systemctl
   --user start smith-worker.service`, then watch one issue flow end to end
   (issue filed → daemon dispatch → worker pushes `agent/...` branch → daemon
   opens the PR as the role → CI green → daemon mechanical backstop merges).

The legacy walkthrough in
[`docs/how-to/run-local-delivery.md`](../docs/how-to/run-local-delivery.md)
describes the superseded per-role pool; provisioning
([`provision-smith-dogfood.md`](../docs/how-to/provision-smith-dogfood.md))
is unchanged — `roles.env` is reused as-is.
