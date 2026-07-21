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
endpoint at `/forgejo/webhook`, and three distinct cadences: dedicated CI-status,
full role-feed, and mechanical automation backstops.

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

### Descendant containment and abrupt worker death

The checked-in worker unit sets `Delegate=yes`. On a cgroup-v2 Linux host this
gives Temper ownership of a writable service subtree. Temper creates the
per-job cgroup before spawning the agent, places the child in it before `exec`,
and creates nested per-tool and per-command cgroups below that job boundary.
Descendants cannot escape cleanup by changing process group or session. Cleanup
attempts TERM, escalates with `cgroup.kill` (or pidfd enumeration), reaps the
direct child, and waits for recursive `populated 0`/empty-membership proof
before accepting tool or job completion.

The preferred Linux backend requires all of the following:

- a unified cgroup-v2 mount and systemd delegation (`Delegate=yes`);
- a writable nested subtree and writable membership controls;
- pidfd support for PID-reuse-safe signaling;
- `cgroup.kill` when available (Temper safely enumerates nested members when it
  is absent).

At worker startup, `worker.containment.startup_capability` reports the cgroup-v2
mount, delegation, nested-subtree writability, `cgroup.kill`, pidfd support,
selected backend, and bounded fallback reason. Managed bash and MCP cleanup is
reported over the attempt-bound lifecycle carrier in both split-agent and
standalone mode, so nested blocked/fallback/completed events retain
worker/job/attempt and owner/tool identity. If delegation is unavailable,
Temper emits `worker.containment.fallback_activated` at warning level and uses
its Linux subreaper/supervisor backend. That fallback tracks and reaps
re-parented descendants across process groups and sessions; it is not the old
process-group-only adapter. Windows workers require nested, kill-on-close Job
Objects and recursive-empty verification. Unsupported platforms fail
containment preparation rather than claiming a descendant-complete guarantee.

Startup probing owns a dedicated `temper` subtree. Each job tree is nested
under a logical-worker and process-boot fence containing the owner's PID and
non-zero kernel start-time identity. If startup cannot establish that fence,
the cgroup backend is unavailable and normal Auto selection uses the Linux
supervisor fallback. Startup preserves every fence whose exact owner is still
live; only a missing owner or a reused PID proves a fence stale enough to
signal. Proven-stale cgroups are killed and removed deepest-first after an
independent empty-tree proof. Legacy, malformed, and uninspectable roots are
retained without signaling. `worker.containment.startup_scavenge` reports
removed, live-protected, and retained counts, bounded retained-path diagnostics,
and omitted counts without command or credential content. Never manually move
unrelated processes into the Temper subtree.

`SIGINT`/`SIGTERM` closes intake, fences every active attempt, requests cleanup,
and waits for task and containment quiescence before worker shutdown returns.
The unit retains `KillMode=control-group` as the abrupt-death backstop: if the
worker is killed, the kernel fails, or cleanup remains blocked beyond
`TimeoutStopSec=5min`, systemd sends SIGKILL to the entire service cgroup. That
forced stop can preserve no application-level cleanup report; after restart,
stale-subtree inspection supplies the available evidence. Increase
`TimeoutStopSec` on hosts where kernel or storage stalls can legitimately delay
cleanup, but do not use `KillMode=process` or remove delegation.

## 5. Register the webhook contract

Register one Forgejo webhook per managed repository with this target:

```text
http://<engine-host>:<engine-port>/forgejo/webhook
```

Use the same HMAC value as the config's named `webhook-secret` credential and
enable issue, pull-request, review/status/CI, label, and push events. Webhooks
are edge-triggered wake hints only: the engine always reloads Forge state before
acting. `ci_poll_cadence_secs` bounds webhook-less terminal red-repair and green-
landing detection; `poll_cadence_secs` remains the full liveness/correctness
backstop. `mechanical_cadence_secs` alone does not discover red engineer repair
work. A `ci_failed` condition is eligible only after every latest-per-name job
for the current head is terminal, so a failure mixed with queued/running work
remains pending. Do not run a separate trigger service.

See `examples/systemd/` for the checked-in config, workflow, credentials, and
unit contract.
