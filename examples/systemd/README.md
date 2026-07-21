# Temper systemd examples

These snippets show the documented split-runtime surface:

- `temper serve engine` for the scheduler/API/webhook process.
- `temper serve worker --pool <name>` for scalable worker pools.
- systemd `LoadCredential=` for the complete example credentials file.

They are examples, not packaging. Copy them to `/etc/systemd/system`, install
`temper` at `/usr/local/bin/temper`, and put the reviewed bundle under
`/etc/temper`.

## Operator lifecycle

Use the same lifecycle for this example as every other deployment:
`temper init -> temper check -> temper plan -> temper apply -> temper serve`.
For the checked-in bundle, the concrete preflight and startup commands are:

```sh
temper --config ./deploy init
# Review or replace the generated files with the checked-in examples.
temper --config examples/systemd/config.example.toml \
  --secrets examples/systemd/credentials.example.toml check --component engine
temper --config examples/systemd/config.example.toml \
  --secrets examples/systemd/credentials.example.toml check \
  --component worker --pool engineers
temper --config examples/systemd/config.example.toml \
  --secrets examples/systemd/credentials.example.toml plan
temper --config examples/systemd/config.example.toml \
  --secrets examples/systemd/credentials.example.toml apply --yes
temper --config examples/systemd/config.example.toml \
  --secrets examples/systemd/credentials.example.toml serve engine
```

In production, run the service units for the final `serve` step rather than the
foreground command.

## Files

- `temper-engine.service` — engine, worker protocol, webhook endpoint, polling,
  and mechanical backstops.
- `temper-worker@.service` — one worker process per `[[worker.pools]]` instance.
- `config.example.toml` — target-era split deployment with named worker pools
  and agent profiles.
- `workflow.example.yaml` — config-relative workflow selected by `[workflow]`.
- `credentials.example.toml` — parseable placeholders for every named secret,
  plus Forge/provider identities retained for migration fallbacks.

## Credentials

The units intentionally omit `--secrets`. systemd sets `CREDENTIALS_DIRECTORY`
from this declaration:

```ini
LoadCredential=credentials.toml:/etc/temper/credentials.toml
```

Temper loads the structured identities and every named Forge, webhook, worker
pool, and provider secret from that file. Operators may move a named secret to
a separate systemd credential later; a same-named file in the credential
directory takes precedence over `[secrets]` without changing `config.toml`.

For shell preflight, pass `--secrets /etc/temper/credentials.toml` explicitly.
`temper config show` reports only secret names and availability, never values.

## Webhook contract

There is no trigger unit. Forgejo webhooks post to the engine at
`/forgejo/webhook`. The payload only wakes the same scan/reconcile path used by
polling; it is never authoritative data. Keep all three backstops configured:

- `ci_poll_cadence_secs` controls the dedicated scan for terminal CI changes
  when Actions-completion webhooks are unavailable. It defaults to 60 seconds;
  set it to `0` only to disable this dedicated scan.
- `poll_cadence_secs` remains the positive, full role-feed correctness backstop
  and defaults to 300 seconds.
- `mechanical_cadence_secs` controls automated queue reconciliation; it does not
  replace the CI poll for red-CI repair discovery.

## Install and start

```sh
install -m 0644 examples/systemd/temper-engine.service /etc/systemd/system/
install -m 0644 examples/systemd/temper-worker@.service /etc/systemd/system/
install -d -m 0750 -o temper -g temper /etc/temper
install -m 0640 -o root -g temper examples/systemd/config.example.toml \
  /etc/temper/config.toml
install -m 0640 -o root -g temper examples/systemd/workflow.example.yaml \
  /etc/temper/workflow.example.yaml
install -m 0600 -o root -g root examples/systemd/credentials.example.toml \
  /etc/temper/credentials.toml

temper --config /etc/temper/config.toml \
  --secrets /etc/temper/credentials.toml check --component engine
for pool in architects engineers reviewers; do
  temper --config /etc/temper/config.toml \
    --secrets /etc/temper/credentials.toml check \
    --component worker --pool "$pool"
done

systemctl daemon-reload
systemctl enable --now temper-engine.service
systemctl enable --now temper-worker@architects.service
systemctl enable --now temper-worker@engineers.service
systemctl enable --now temper-worker@reviewers.service
```
