# Temper systemd examples

These snippets show the documented runtime surface:

- `temper serve engine` for the scheduler/API/webhook process.
- `temper serve worker --pool <name>` for scalable worker pools.
- systemd `LoadCredential=` for `credentials.toml` and named secrets.

They are examples, not packaging. Copy them to `/etc/systemd/system`, install
`temper` at `/usr/local/bin/temper`, and put edited config/secrets under
`/etc/temper`.

## Files

- `temper-engine.service` — engine, worker protocol, webhook endpoint, polling
  and mechanical backstops.
- `temper-worker@.service` — one worker process per `[[worker.pools]]`
  instance.
- `config.example.toml` — split-runtime config with named worker pools.
- `credentials.example.toml` — placeholder credentials loaded via systemd.

## Credentials

The units intentionally omit `--secrets`. systemd sets `CREDENTIALS_DIRECTORY`
when `LoadCredential=` is used, and Temper reads that directory as the selected
secret source. The examples load both:

```ini
LoadCredential=credentials.toml:/etc/temper/credentials.toml
LoadCredential=webhook-secret:/etc/temper/webhook-secret
```

`credentials.toml` carries structured forge/provider credentials. The separate
`webhook-secret` file satisfies the config's `webhook_secret = "webhook-secret"`
reference. Loading both credentials into both engine and worker units keeps the
same resolved bundle valid for runtime start. For shell preflight, pass
`--secrets /etc/temper` to use `/etc/temper` as the same directory secret
source.

## Trigger contract

There is no trigger unit. Forgejo webhooks post to the engine at
`/forgejo/webhook`. The payload only wakes the same scan/reconcile path used by
polling; it is never authoritative data. Keep `poll_cadence_secs` and
`mechanical_cadence_secs` configured as the backstop.

## Example commands

```sh
install -m 0644 examples/systemd/temper-engine.service /etc/systemd/system/
install -m 0644 examples/systemd/temper-worker@.service /etc/systemd/system/
install -d -m 0750 -o temper -g temper /etc/temper
install -m 0640 -o root -g temper examples/systemd/config.example.toml \
  /etc/temper/config.toml
install -m 0600 -o root -g root examples/systemd/credentials.example.toml \
  /etc/temper/credentials.toml
printf '%s' '<shared-hmac-secret>' >/etc/temper/webhook-secret
chmod 0600 /etc/temper/webhook-secret

temper --config /etc/temper --secrets /etc/temper check --component engine
temper --config /etc/temper --secrets /etc/temper check \
  --component worker --pool engineers

systemctl daemon-reload
systemctl enable --now temper-engine.service
systemctl enable --now temper-worker@engineers.service
```
