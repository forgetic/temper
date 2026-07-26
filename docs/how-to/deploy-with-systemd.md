# Deploy Temper with systemd

Use this recipe for either an all-in-one standalone deployment or a split
engine/worker deployment on Linux. The public startup surface is `temper serve`;
the older `temper daemon` command is compatibility-only. Select one topology
for a state directory—never run standalone beside split services over the same
state.

## 1. Prepare, check, plan, and apply the bundle

The production lifecycle is
`temper init -> temper check -> temper plan -> temper apply -> temper serve`.
Create and review the bundle before inspecting the Forge reconciliation plan:

```sh
temper --config ./deploy init
$EDITOR ./deploy/config.toml ./deploy/credentials.toml ./deploy/workflow.yaml
temper --config ./deploy check --component standalone # all-in-one option
temper --config ./deploy check --component engine     # split option
temper --config ./deploy check --component worker --pool engineers
temper --config ./deploy plan
temper --config ./deploy apply --yes
```

`temper plan` is read-only; `temper apply` is Forge-mutating. Production
automation should keep check, plan, and apply separate.

Forgejo 16.0.1 is the minimum supported release. For an existing persistent
service, do not deploy this API-only binary as an ordinary application restart:
follow the [Forgejo 16 migration runbook](migrate-forgejo-16-api-ci.md). Prove
the Bench fixture and merge first; an operator then backs up, rehearses,
migrates, and proves the persistent service with the previous compatible Temper
deployment. Only afterward remove the obsolete `ci_user` setting and restart the
new binary. No repository command in this guide migrates the provider service.

## 2. Use systemd credentials for secrets

Install non-secret config and workflow files under `/etc/temper`. Keep secrets
in `/etc/temper/credentials.toml`, mode `0600`, owned by root or the Temper
service user. The checked-in units use:

```ini
LoadCredential=credentials.toml:/etc/temper/credentials.toml
```

systemd copies that file into `$CREDENTIALS_DIRECTORY`; Temper reads the
credential directory automatically when `--secrets` is omitted. A deployment
may split selected names into one credential file per secret; named files
override same-named `[secrets]` entries. For a shell preflight, pass
`--secrets /etc/temper/credentials.toml` explicitly. `temper config show`
reports references and availability without revealing values.

## 3. Run standalone

The checked example config declares a `local` worker pool, so it is directly
compatible with the standalone unit:

```sh
install -m 0644 examples/systemd/temper-standalone.service \
  /etc/systemd/system/
systemctl daemon-reload
temper --config /etc/temper/config.toml \
  --secrets /etc/temper/credentials.toml check --component standalone
systemctl enable --now temper-standalone.service
# Equivalent foreground command:
temper --config /etc/temper/config.toml \
  --secrets /etc/temper/credentials.toml serve standalone
```

The standalone process owns the engine, local worker, native agent, webhook,
and polling/mechanical backstops. Its `[deployment]` setting
`standalone_shutdown_budget_secs = 30` is one absolute duration measured from
SIGINT/SIGTERM receipt. `temper config show` prints the resolved budget. Config
resolution rejects a budget that does not strictly exceed the worker graceful
and forced cancellation graces plus the fixed 5-second HTTP-drain and 5-second
final emergency-kill allowances.

The unit sets `TimeoutStopSec=45s`: it is strictly greater than Temper's default
30-second budget and preserves an explicit 15-second safety margin for service
manager scheduling and final accounting. Temper's deadline terminator is
core-dump-free, so core generation cannot consume that margin. When tuning,
increase the internal budget first, rerun `temper check`, and then set
`TimeoutStopSec` strictly larger (preferably retaining at least this margin).
Never configure `TimeoutStopSec` at or below Temper's internal budget.

### Proof-based shutdown versus bounded process-loss recovery

Ordinary ownership loss remains proof-based and fail closed. The attempt fence
closes before cancellation; result, Forge-context, submit, workspace, git,
push, and other attempt effects are rejected. Temper retains the registry
entry, heartbeat membership, and permit until it has both recursive descendant
emptiness/direct-child reap and terminal-trace acknowledgement. There is no
ordinary timeout that fabricates `AttemptQuiesced` or permits stale publication.

Stopping the standalone service adds a bounded process-level contract:

1. close daemon claim, result, Forge-context, and Forge-application admission;
2. fence every active attempt and begin HTTP drain;
3. consume one deadline across graceful/forced/hard worker cancellation,
   admitted daemon work, trace retention, exact joined-assignment release, and
   HTTP drain;
4. reserve the final five seconds for independent emergency KILL and an
   immediate core-dump-free process exit.

If every proof arrives in time, only exact joined attempts are released and
`standalone.shutdown.summary` emits `disposition=graceful_exit`. If any proof is
still blocked, Temper emits `disposition=bounded_crash_handoff`, retains all
unproven durable assignments and the durable trace spool, invokes the
attempt-owned out-of-band termination authority, and exits immediately with
status 70. The termination primitive generates no core, does not unwind or run
Rust owner drops, invokes no C/Rust exit handlers, and flushes no userspace
buffers. This is process-loss recovery, not successful local quiescence: it does
not synthesize descendant proof, terminal-trace acknowledgement, a result,
capacity release, or normal assignment release.

`Restart=on-failure` starts a replacement after the bounded handoff. Startup
stages prior-boot assignments with dispatch closed, reattaches only exact live
attempts, converges unreattached orphans from fresh Forge state, replays durable
results, and forwards retained trace-spool records. Existing attempt fences and
durable claim checks reject late results or Forge operations from the old
attempt, while feed/startup convergence makes abandoned work recoverable once.

### Shutdown diagnostics

A blocked deadline emits `standalone.shutdown.blocker` events followed by the
terminal `standalone.shutdown.summary`. The closed `blocker_kind` vocabulary is
`containment`, `terminal_trace_ack`, `result_delivery`, `component_task`, and
`registry_state`. Each bounded/redacted event carries available worker, job,
and attempt IDs; owner scope/name; owner root; root PID and sampled survivor
PIDs; containment phase or trace run/sequence; first-seen time and increasing
age; escalation stage; deadline remaining; and occurrence/omission counts.
A zero/`unknown` field means that evidence was unavailable, not that no process
or blocker existed.

### Descendant containment and systemd backstop

The standalone and worker units set `Delegate=yes`. On cgroup-v2 Linux this
provides a writable service subtree. Temper creates per-job and nested
per-tool/command ownership cgroups before exec. Cleanup uses `cgroup.kill` or
pidfd-safe nested enumeration, reaps the direct child, and waits for
recursive-empty evidence. When delegation is unavailable, the Linux
subreaper/supervisor fallback owns and reaps re-parented descendants across
process groups and sessions; Windows uses nested kill-on-close Job Objects.

Both units use `KillMode=control-group`. This is the external abrupt-death
backstop for kernel failure, an early crash, or failure before Temper arms its
watchdog. Operators must not use `KillMode=process`: killing only the main PID
can strand agents, MCP servers, and managed commands and invalidates the
containment guarantee. Removing `Delegate=yes` also forfeits the preferred
cgroup-v2 backend.

At startup, `worker.containment.startup_capability` reports delegation,
subtree, `cgroup.kill`, pidfd, selected backend, and bounded fallback reason.
Cleanup events report nested owner evidence. Proven-stale startup cgroups are
killed and removed only after process-incarnation checks; malformed, live, or
uninspectable roots are retained rather than signaled.

## 4. Run split engine and worker pools

For split operation, copy `temper-engine.service` and
`temper-worker@.service`, then start one worker template instance per configured
pool:

```sh
systemctl enable --now temper-engine.service
systemctl enable --now temper-worker@architects.service
systemctl enable --now temper-worker@engineers.service
systemctl enable --now temper-worker@reviewers.service
# Equivalent foreground worker command:
temper --config /etc/temper/config.toml \
  --secrets /etc/temper/credentials.toml serve worker --pool engineers
```

The `%i` instance becomes `--pool %i`. Use `--capacity` in a local override only
for a host-specific limit no larger than pool policy.

A split worker intentionally retains the ordinary proof-based shutdown
semantics described above: it may wait past its graceful and forced graces while
recursive-empty or terminal-trace proof is unavailable. Its example
`TimeoutStopSec=5min` and `KillMode=control-group` are an external crash
backstop, not Temper's standalone bounded-handoff path. Increase that timeout
when legitimate kernel/storage stalls require it; do not weaken the kill mode.

The engine owns queue scheduling, worker protocol, webhook endpoint, and three
distinct cadences: dedicated CI-status, full role-feed, and mechanical
automation backstops.

## 5. Register the webhook contract

Register one Forgejo webhook per managed repository at:

```text
http://<engine-or-standalone-host>:<port>/forgejo/webhook
```

Use the configured webhook credential and enable issue, pull-request,
review/status/CI, label, and push events. Webhooks are wake hints only; the
engine always reloads Forge state. `ci_poll_cadence_secs` bounds webhook-less
terminal CI detection, while the positive `ci_missing_grace_secs` bounds how
long an exact current head may have no matching CI before safe parking becomes
actionable. If `ci_poll_cadence_secs = 0`, both missing-CI detection and parking
are inactive even though the grace remains configured. `poll_cadence_secs`
remains the full liveness/correctness backstop, and `mechanical_cadence_secs`
does not replace either. Do not install a separate trigger service.

See `examples/systemd/` for the checked config, workflow, credentials, and all
three unit contracts.
