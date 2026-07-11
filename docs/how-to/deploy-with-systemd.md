# Deploy Temper with systemd

Use this recipe for a split engine/worker deployment on one or more Linux hosts.
The public startup surface is `temper serve`; the older `temper daemon` command
is compatibility-only.

## 1. Prepare, check, plan, and apply the bundle

The production operator lifecycle is explicit:
`temper init -> temper check -> temper plan -> temper apply -> temper serve`.
Create a bundle with the interactive flow, review it, and run offline checks for
the engine and every worker pool before inspecting the Forge reconciliation
plan:

```sh
temper --config ./deploy init
$EDITOR ./deploy/config.toml ./deploy/credentials.toml ./deploy/workflow.yaml
temper --config ./deploy check --component engine
temper --config ./deploy check --component worker --pool engineers
temper --config ./deploy plan
temper --config ./deploy apply --yes
```

`temper plan` performs read-only Forge inspection; `temper apply` is the
forge-mutating step. For demo-only runs, `temper init --apply --yes` combines the
local write and apply steps. Production automation should keep check, plan, and
apply separate.

## 2. Use systemd credentials for secrets

Install non-secret config and workflow files under `/etc/temper`. Keep secrets
in `/etc/temper/credentials.toml`, mode `0600`, owned by root or the Temper
service user. The checked-in example stores every named Forge, webhook, pool,
and provider secret in this TOML file.

The example units use `LoadCredential=credentials.toml:/etc/temper/credentials.toml`.
systemd copies that file into `$CREDENTIALS_DIRECTORY`; Temper reads the
credential directory automatically when `--secrets` is omitted. A deployment
may instead split selected names into one systemd credential file per secret;
named files override same-named `[secrets]` entries during migration.

For preflight checks outside systemd, pass the file explicitly:

```sh
temper --config /etc/temper/config.toml \
  --secrets /etc/temper/credentials.toml check --component engine
temper --config /etc/temper/config.toml \
  --secrets /etc/temper/credentials.toml check \
  --component worker --pool engineers
```

## 3. Run the engine

Copy `examples/systemd/temper-engine.service` to `/etc/systemd/system/`, edit
paths if needed, then:

```sh
systemctl daemon-reload
systemctl enable --now temper-engine.service
# Equivalent foreground command:
temper --config /etc/temper/config.toml \
  --secrets /etc/temper/credentials.toml serve engine
```

The engine owns queue scheduling, the worker protocol, the Forgejo webhook
endpoint at `/forgejo/webhook`, and poll/mechanical backstops.

## 4. Run worker pools

Declare worker pools in `[[worker.pools]]` and start one template instance per
pool:

```sh
systemctl enable --now temper-worker@architects.service
systemctl enable --now temper-worker@engineers.service
systemctl enable --now temper-worker@reviewers.service
# Equivalent foreground command for one pool:
temper --config /etc/temper/config.toml \
  --secrets /etc/temper/credentials.toml serve worker --pool engineers
```

The `%i` instance name becomes `--pool %i`. Use `--capacity` in a local override
for a host-specific concurrency limit; otherwise the resolved pool policy is
used.

## 5. Register the webhook contract

Register one Forgejo webhook per managed repository with this target:

```text
http://<engine-host>:<engine-port>/forgejo/webhook
```

Use the same HMAC value as the config's named `webhook-secret` credential and
enable issue, pull-request, review/status/CI, label, and push events. Webhooks
are edge-triggered wake hints only: the engine always reloads Forge state before
acting, and periodic polling remains the liveness/correctness backstop. Do not
run a separate trigger service.

See `examples/systemd/` for the checked-in config, workflow, credentials, and
unit contract.
