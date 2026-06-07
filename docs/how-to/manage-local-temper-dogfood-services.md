# Manage local Temper dogfood services

Use this guide to inspect and manage the local Temper dogfood worker pool for
`ai/temper` on a dogfood host.

## When to use this guide

Use this guide when you see `temper-*` systemd user services and need to know
which ones belong to the local `ai/temper` dogfood pool.

These units are separate from any other Smith or Temper worker pool on the same
host. They use separate unit names and separate config and state locations. Do
not assume paths or service names from another local pool apply here.

## List the Temper dogfood units

List active or loaded `temper-*` user units:

```sh
systemctl --user list-units 'temper-*'
```

List installed unit files and their enablement state:

```sh
systemctl --user list-unit-files 'temper-*'
```

In `list-units` output, check `ACTIVE` and `SUB` first. `active` with a
`running` substate means the service is currently running. `failed` means the
unit entered a failure state and needs inspection before restart. In
`list-unit-files` output, `enabled` means the unit starts with the user manager;
`disabled` means it does not start automatically.

## Inspect a service

Use the exact unit name from the list commands, for example
`temper-architect.service`.

```sh
systemctl --user status <unit>
systemctl --user cat <unit>
systemctl --user show <unit> \
  --property=FragmentPath,DropInPaths,ExecStart,EnvironmentFiles,WorkingDirectory
```

`status` shows the current state and recent log lines. `cat` prints the unit
file and drop-ins. `show` prints the properties systemd is using after it has
loaded the unit.

Treat `cat` and `show` as the source of truth for the service's actual paths,
arguments, environment files, and working directory on that host.

## Read logs

Read recent logs for a unit:

```sh
journalctl --user -u <unit> -n 100
```

Follow logs while the service runs:

```sh
journalctl --user -u <unit> -f
```

Limit logs to the current boot:

```sh
journalctl --user -u <unit> -b
```

`temper-worker` tick logs include fields documented in
[Production worker](../reference/production-worker.md). Use that reference for
worker runtime diagnostics after you have identified the unit and log stream.

## Manage services

Restart a service after changing config or after diagnosing a transient issue:

```sh
systemctl --user restart <unit>
```

Stop or start a service explicitly:

```sh
systemctl --user stop <unit>
systemctl --user start <unit>
```

Disable and stop a service, or enable and start it again:

```sh
systemctl --user disable --now <unit>
systemctl --user enable --now <unit>
```

Clear a recorded failed state after diagnosis:

```sh
systemctl --user reset-failed <unit>
```

Stopping or disabling a dogfood worker pauses that role's local processing until
the unit is started again.

## Find config and state directories

The Temper dogfood pool intentionally uses config and state paths separate from
other local worker pools. Discover the real paths from the unit definition and
environment files instead of assuming a universal host path.

```sh
systemctl --user cat <unit>
systemctl --user show <unit> \
  --property=EnvironmentFiles,WorkingDirectory,ExecStart
```

Check `EnvironmentFiles`, `WorkingDirectory`, and `ExecStart` for config flags,
state directories, and wrapper scripts. Common XDG-style places to check include
these examples, but they are not guarantees:

- `~/.config/temper/`
- `~/.local/state/temper/`
- `~/.local/share/temper/`

If the unit references an environment file, inspect it read-only:

```sh
sed -n '1,160p' <env-file>
```

Do not paste secrets, tokens, private URLs, or other sensitive values from unit
files, environment files, or logs into issues or chat transcripts.

## Quick health checklist

- Units are present and active where expected:

  ```sh
  systemctl --user list-units 'temper-*'
  ```

- No `temper-*` user units are failed:

  ```sh
  systemctl --user --failed 'temper-*'
  ```

- Recent logs show completed ticks rather than repeated errors.
- Unit paths and config paths point at the Temper dogfood pool, not another
  repository's worker pool.
