# Deploy Temper with systemd

Use this recipe for a split engine/worker deployment on one or more Linux hosts.
The public startup surface is `temper serve`; the older `temper daemon` command
is compatibility-only.

## 1. Prepare and validate the bundle

Create a bundle with the interactive flow, or copy a reviewed config into
`/etc/temper`:

```sh
temper --config ./deploy init
$EDITOR ./deploy/config.toml ./deploy/credentials.toml ./deploy/workflow.yaml
temper --config ./deploy check --component engine
temper --config ./deploy check --component worker --pool engineers
```

`temper apply` is the forge-mutating step. Review the generated config,
workflow, and provisioning-plan inputs first, then run:

```sh
temper --config ./deploy apply --yes
```

For demo-only runs, `temper init --apply --yes` combines the local write and the
apply step. Production docs should keep the review/check/apply steps explicit.

## 2. Use systemd credentials for secrets

Install non-secret config as `/etc/temper/config.toml`. Keep secrets in
`/etc/temper/credentials.toml` and named files such as
`/etc/temper/webhook-secret`, mode `0600`, owned by root or the Temper service
user.

The example units use `LoadCredential=`. systemd copies those files into
`$CREDENTIALS_DIRECTORY`; Temper reads that directory automatically when
`--secrets` is omitted. A directory credential source may contain either a
legacy `credentials.toml` file, one file per named secret, or both.

For preflight checks outside systemd, pass the same directory as an explicit
secret source so `credentials.toml` and named secret files are loaded together:

```sh
temper --config /etc/temper --secrets /etc/temper check --component engine
temper --config /etc/temper --secrets /etc/temper check \
  --component worker --pool engineers
```

## 3. Run the engine

Copy `examples/systemd/temper-engine.service` to `/etc/systemd/system/`, edit
paths if needed, then:

```sh
systemctl daemon-reload
systemctl enable --now temper-engine.service
```

The engine owns queue scheduling, the worker protocol, the Forgejo webhook
endpoint at `/forgejo/webhook`, and poll/mechanical backstops.

## 4. Run worker pools

Declare worker pools in `[[worker.pools]]` and start one template instance per
pool:

```sh
systemctl enable --now temper-worker@engineers.service
systemctl enable --now temper-worker@reviewers.service
```

The `%i` instance name becomes `--pool %i`. Use `--capacity` in the unit for a
host-local concurrency override; otherwise the resolved pool/default capacity is
used.

## 5. Register the trigger contract

Register one Forgejo webhook per managed repository with this target:

```text
http://<engine-host>:<engine-port>/forgejo/webhook
```

Use the same HMAC secret as the `webhook-secret` systemd credential and enable
issue, pull-request, review/status/CI, label, and push events. Webhooks are
edge-triggered wake hints only: the engine always reloads Forge state before
acting, and periodic polling remains the liveness/correctness backstop. Do not
run a separate `temper serve trigger` service.

See `examples/systemd/` for complete unit and config snippets.
