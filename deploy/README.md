# temper-daemon deployment assets

This directory contains the repo-tracked deployment surface for the consolidated
`temper-daemon` user service. Live operator config and state stay outside the
checkout under `~/.config/temper`, `~/.config/systemd/user`, `~/.local/bin`, and
`~/.local/state`; secrets are never tracked here.

## Layout

```text
deploy/
├── bin/
│   └── temper-daemon-launcher       # ExecStart shim; maps roles.env to FORGEJO_*
├── config/
│   └── daemon.env                   # no-secrets operator config template
├── systemd/
│   └── temper-daemon.service        # systemd --user unit template
├── install.sh                       # idempotent local installer
└── README.md
```

## What the daemon owns

`temper-daemon` is the Forge-facing control plane in the consolidated two-tier
topology. It receives webhook hints, runs the poll backstop, scans configured
repositories, enqueues work for long-polling worker processes, gates applies with
leases, applies role-attributed results through Forgejo, and can run the optional
mechanical backstop. The worker tier connects to the daemon HTTP API exposed on
`DAEMON_BIND`; workers do not call the Forge API directly.

The daemon holds Forge **API** credentials in its environment. The launcher maps
the provisioned default bot identity from `~/.config/temper/secrets/roles.env`
onto the generic `FORGEJO_URL` and `FORGEJO_ACCESS_TOKEN` variables consumed by
`ForgejoConfig::from_env`, and passes optional web-UI credentials through as
`FORGEJO_USERNAME`/`FORGEJO_PASSWORD` for the ADR 0019 CI-read fallback used by
the mechanical backstop. Per-role Forge API tokens such as
`TEMPER_FORGEJO_TOKEN_ENGINEER` pass through unchanged so role-attributed applies
can use them when present. Git push credentials remain with the worker tier and
are not part of this daemon deployment.

The worker tier (`smith-worker`) is owned by and deployed from the `ai/smith`
repository's `deploy/` directory.

## Install

From the repository root:

```sh
deploy/install.sh
```

The installer builds the debug `temper-daemon` binary with
`cargo build -j2 --bin temper-daemon` unless `TEMPER_SKIP_BUILD=1` is set, then
installs:

- `target/debug/temper-daemon` to `~/.local/bin/`
- `deploy/bin/temper-daemon-launcher` to `~/.local/bin/`
- `deploy/systemd/temper-daemon.service` to `~/.config/systemd/user/`
- `deploy/config/daemon.env` to `~/.config/temper/daemon.env` only when that
  file does not already exist

The installer does not provision Forgejo, write secrets, start, or enable the
unit. After installing, ensure `~/.config/temper/secrets/roles.env` exists,
review `~/.config/temper/daemon.env`, then run:

```sh
systemctl --user daemon-reload && systemctl --user start temper-daemon.service
```

Use `systemctl --user enable temper-daemon.service` separately if the unit should
start automatically for the user session.
