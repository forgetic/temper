# temper engine-service deployment assets

This directory contains the repo-tracked deployment surface for the temper
**engine** service (`temper daemon --service engine`). Live operator config and
state stay outside the checkout under `~/.config/temper`, `~/.config/systemd/user`,
`~/.local/bin`, and `~/.local/state`; secrets are never tracked here.

## Layout

```text
deploy/
├── bin/
│   └── temper-daemon-launcher       # ExecStart shim; maps roles.env to FORGEJO_*
├── config/
│   └── config.toml                  # no-secrets deployment config template
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

The engine holds Forge **API** credentials in its environment. Deployment
settings (forge URL, repositories, roles, cadences, bind) live in the non-secret
`~/.config/temper/config.toml`; secrets layer over it from the environment. The
launcher maps the provisioned default bot identity from
`~/.config/temper/secrets/roles.env` onto the `FORGEJO_ACCESS_TOKEN` the config
resolver reads (env overrides the file), and passes optional web-UI credentials
through as `FORGEJO_USERNAME`/`FORGEJO_PASSWORD` for the ADR 0019 CI-read
fallback used by the mechanical backstop. Per-role Forge API tokens such as
`TEMPER_FORGEJO_TOKEN_ENGINEER` pass through unchanged so role-attributed applies
can use them when present. Git push credentials remain with the worker tier and
are not part of this engine deployment.

The worker tier (`smith-worker`) is owned by and deployed from the `ai/smith`
repository's `deploy/` directory.

## Install

From the repository root:

```sh
deploy/install.sh
```

The installer builds the debug `temper` binary with `cargo build -j2 --bin temper`
unless `TEMPER_SKIP_BUILD=1` is set, then installs:

- `target/debug/temper` to `~/.local/bin/`
- `deploy/bin/temper-daemon-launcher` to `~/.local/bin/`
- `deploy/systemd/temper-daemon.service` to `~/.config/systemd/user/`
- `deploy/config/config.toml` to `~/.config/temper/config.toml` only when that
  file does not already exist

The installer does not provision Forgejo, write secrets, start, or enable the
unit. After installing, ensure `~/.config/temper/secrets/roles.env` exists,
review `~/.config/temper/config.toml` (run `temper config validate`), then run:

```sh
systemctl --user daemon-reload && systemctl --user start temper-daemon.service
```

Use `systemctl --user enable temper-daemon.service` separately if the unit should
start automatically for the user session.

## Logging

The daemon's tracing init is environment-aware: under systemd the unit sets
`StandardOutput=journal`/`StandardError=journal`, so `JOURNAL_STREAM` is present
in the daemon's environment and logs are emitted as structured entries through
`tracing-journald` (native journal priorities, no ANSI escapes). Running the same
binary by hand in a terminal instead writes colored, human-readable lines to
stderr. Same binary, no flags — the destination is detected automatically.

### Level control: `RUST_LOG`

`RUST_LOG` is the single verbosity knob. The unit ships a default of
`RUST_LOG=info`. To change it, set `RUST_LOG` in the optional drop-in env file
`~/.config/temper/daemon.env` (the unit loads it via
`EnvironmentFile=-%h/.config/temper/daemon.env`; the `-` prefix makes it
optional), then restart:

```sh
echo 'RUST_LOG=debug' >> ~/.config/temper/daemon.env
systemctl --user restart temper-daemon.service
```

`RUST_LOG` accepts the standard `EnvFilter` syntax, so per-target levels work too
(e.g. `RUST_LOG=info,temper_engine=debug`). Omitting it falls back to `info`.

### Reading the journal

```sh
# Follow all daemon entries.
journalctl --user -u temper-daemon -f

# Errors only. tracing levels map to journal priorities, so priority filtering
# works: -p err shows ERROR, -p warning shows WARN and above.
journalctl --user -p err -u temper-daemon

# Query by syslog identifier (set via SyslogIdentifier=temper-daemon in the
# unit), useful for cross-unit or structured queries.
journalctl --user SYSLOG_IDENTIFIER=temper-daemon
```

Drop `--user` when inspecting a system-scope deployment.

### Readiness

The unit is `Type=simple`: systemd treats the service as started once the process
is forked, and the daemon logs a readiness line after it binds. A future
`Type=notify` would let the daemon call `sd_notify(READY=1)` after binding so
systemd tracks readiness precisely; that is not wired up yet (see the TODO in
`systemd/temper-daemon.service`).
