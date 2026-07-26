# Temper systemd examples

These snippets cover both supported deployment shapes:

- `temper-standalone.service` runs `temper serve standalone` (engine, worker, and
  agent in one process).
- `temper-engine.service` plus `temper-worker@.service` run the split engine and
  named worker pools.
- systemd `LoadCredential=` supplies the complete example credentials file.

Choose one shape; do not start the standalone and split units over the same
state directory. These are examples, not packaging. Install `temper` at
`/usr/local/bin/temper` and put the reviewed bundle under `/etc/temper`.

## Operator lifecycle

Use the normal lifecycle before enabling either shape:
`temper init -> temper check -> temper plan -> temper apply -> temper serve`.
For the checked-in bundle, the concrete commands are:

```sh
temper --config ./deploy init
# Review or replace the generated files with the checked-in examples.
temper --config examples/systemd/config.example.toml \
  --secrets examples/systemd/credentials.example.toml check --component standalone
temper --config examples/systemd/config.example.toml \
  --secrets examples/systemd/credentials.example.toml check --component engine
temper --config examples/systemd/config.example.toml \
  --secrets examples/systemd/credentials.example.toml check \
  --component worker --pool engineers
temper --config examples/systemd/config.example.toml \
  --secrets examples/systemd/credentials.example.toml plan
temper --config examples/systemd/config.example.toml \
  --secrets examples/systemd/credentials.example.toml apply --yes
```

In production, use the selected service unit for the final `serve` step. Forgejo
16.0.1 is the minimum supported release, and engine/standalone CI observation
uses the configured token only. Existing persistent services must complete the
operator-owned
[Forgejo 16 migration runbook](../../docs/how-to/migrate-forgejo-16-api-ci.md)
before this API-only binary starts: prove the fixture and feature, migrate and
prove Forgejo with the previous compatible deployment, then remove `ci_user`
from deployed configuration and restart.

## Files and configuration contract

- `temper-standalone.service` — checked all-in-one unit using the public
  `serve standalone` command.
- `temper-engine.service` — scheduler, worker protocol, webhook endpoint,
  polling, and mechanical backstops.
- `temper-worker@.service` — one worker process per split pool.
- `config.example.toml` — named split pools plus a `local` pool, so the same
  reviewed bundle is also compatible with standalone selection.
- `workflow.example.yaml` — config-relative workflow selected by `[workflow]`.
- `credentials.example.toml` — parseable placeholders for every named secret,
  plus token-only Forge/provider identities retained for migration fallbacks.

The config pins `deployment.standalone_shutdown_budget_secs = 30`. It is one
absolute interval measured from SIGINT/SIGTERM receipt, not a fresh timeout per
shutdown phase. Temper validates that it strictly exceeds the configured
worker graceful and forced cancellation graces plus the fixed 5-second HTTP-
drain and 5-second final emergency-kill allowances. Confirm the resolved value
with `temper config show` after every change.

`temper-standalone.service` sets `TimeoutStopSec=45s`, strictly greater than the
30-second internal budget. The explicit 15-second safety margin covers service
manager scheduling and final process accounting without making systemd the
normal termination mechanism. Deadline termination is core-dump-free, so core
generation cannot consume that margin. If the internal budget is tuned, keep
`TimeoutStopSec` strictly larger (preferably by at least the same margin). Never
set it at or below Temper's budget.

## Credentials

The units intentionally omit `--secrets`. systemd sets `CREDENTIALS_DIRECTORY`
from this declaration:

```ini
LoadCredential=credentials.toml:/etc/temper/credentials.toml
```

Temper loads the structured identities and named Forge, webhook, worker-pool,
and provider secrets from that file. Operators may move a named secret to a
separate systemd credential later; a same-named file in the credential
directory takes precedence over `[secrets]` without changing `config.toml`.
For shell preflight, pass `--secrets /etc/temper/credentials.toml` explicitly.
`temper config show` reports only secret names and availability, never values.

## Standalone shutdown contract

The standalone unit requires both of these settings:

```ini
Delegate=yes
KillMode=control-group
```

`Delegate=yes` supplies the writable cgroup-v2 subtree used for attempt-owned
agent, MCP, managed-command, and tool containment. Temper prefers cgroup-v2
with pidfd-safe signaling and `cgroup.kill`; if delegation is unavailable its
Linux subreaper/supervisor fallback owns and reaps re-parented descendants.
Ordinary cleanup remains proof-based on every backend: direct-child reap and
recursive-empty evidence must arrive before the attempt can quiesce.

On ordinary ownership loss, there is no fail-open timeout. Temper keeps the
attempt fence, registry entry, heartbeat membership, and permit until both
descendant proof and terminal-trace acknowledgement complete. It then records
the cancellation and releases capacity in the established order.

Standalone process shutdown adds a separate process-loss boundary. It first
fences claims, results, assignment-scoped Forge context, Forge application, and
every active attempt. If all admitted operations, attempts, terminal traces,
assignment release, trace retention, and HTTP drain join within the one budget,
`standalone.shutdown.summary` reports `disposition=graceful_exit` and the
process exits normally. If proof is still blocked, Temper retains the durable
assignments and trace spool, reports `disposition=bounded_crash_handoff`, issues
out-of-band emergency descendant termination, and immediately exits with the
distinct non-zero status 70. This core-dump-free primitive does not unwind, run
Rust owner drops, invoke C/Rust exit handlers, or flush userspace buffers. The
non-zero exit lets `Restart=on-failure` start a fresh process; startup assignment
staging, orphan convergence, outbox/result replay, and trace-spool recovery
converge the retained work. Deadline expiry is not `AttemptQuiesced`, cleanup
proof, result publication, or successful assignment release.

Inspect `standalone.shutdown.blocker` records before the terminal summary.
`blocker_kind` is one of `containment`, `terminal_trace_ack`, `result_delivery`,
`component_task`, or `registry_state`. Records include bounded worker/job/
attempt identity, owner scope/name, root PID and survivor PIDs when known,
containment phase or trace run/sequence, first-seen time, increasing blocker
age, escalation stage, deadline remaining, and occurrence/omission counts.
Values are bounded and credential-like text is redacted.

`KillMode=control-group` remains the last external backstop for kernel failure,
a Temper crash before its watchdog is armed, or another abrupt process loss.
Do not use `KillMode=process`: that can leave agents, MCP servers, or nested
commands alive after the main PID is gone and invalidates the containment
contract.

## Webhook contract

There is no trigger unit. Forgejo webhooks post to the engine or standalone at
`/forgejo/webhook`. A payload only wakes the same scan/reconcile path used by
polling; it is never authoritative data. Keep all three backstops configured:

- `ci_poll_cadence_secs` bounds webhook-less terminal CI detection;
- positive `ci_missing_grace_secs` bounds how long exact-head CI may remain
  absent before safe parking is actionable;
- `poll_cadence_secs` remains the positive full role-feed backstop;
- `mechanical_cadence_secs` controls automated queue reconciliation.

A `ci_failed` queue match is current-head and latest-per-job-name: every latest
job must be terminal and at least one must not have succeeded. If
`ci_poll_cadence_secs = 0`, terminal-CI acceleration, missing-current-head
detection, and missing-CI parking are inactive even though the grace remains
configured.

## Install and start

Install the shared bundle and all units you may select:

```sh
install -m 0644 examples/systemd/temper-standalone.service /etc/systemd/system/
install -m 0644 examples/systemd/temper-engine.service /etc/systemd/system/
install -m 0644 examples/systemd/temper-worker@.service /etc/systemd/system/
install -d -m 0750 -o temper -g temper /etc/temper
install -m 0640 -o root -g temper examples/systemd/config.example.toml \
  /etc/temper/config.toml
install -m 0640 -o root -g temper examples/systemd/workflow.example.yaml \
  /etc/temper/workflow.example.yaml
install -m 0600 -o root -g root examples/systemd/credentials.example.toml \
  /etc/temper/credentials.toml

systemctl daemon-reload
```

For standalone:

```sh
temper --config /etc/temper/config.toml \
  --secrets /etc/temper/credentials.toml check --component standalone
systemctl enable --now temper-standalone.service
```

Or, for split operation:

```sh
temper --config /etc/temper/config.toml \
  --secrets /etc/temper/credentials.toml check --component engine
for pool in architects engineers reviewers; do
  temper --config /etc/temper/config.toml \
    --secrets /etc/temper/credentials.toml check \
    --component worker --pool "$pool"
done
systemctl enable --now temper-engine.service
systemctl enable --now temper-worker@architects.service
systemctl enable --now temper-worker@engineers.service
systemctl enable --now temper-worker@reviewers.service
```
