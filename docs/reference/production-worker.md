# Production worker runtime

In the current two-tier deployment, `temper serve engine` owns webhook intake,
poll backstops, and queue scheduling while one or more `temper serve worker`
processes long-poll it for jobs. Forgejo webhooks are posted to the
engine/standalone HTTP surface at `POST /forgejo/webhook` when `[engine]
webhook_secret` or `webhook_secret_file` is configured; there is no separate
`temper serve trigger` process. The older per-role worker, mechanical worker,
and standalone daemon wording below remains useful for legacy or test-only
operation, but operator-facing docs should prefer `serve engine` / `serve
worker`.

This page records the operator-visible knobs on the Forgejo `temper-worker`
binary. The deployable entrypoint lives in the root `temper` package and
delegates to `crates/temper-worker`; its legacy/internal wake socket support is
shared through `crates/temper-wake`, and optional local-git edits are bound
through `crates/temper-coding-workspace`.

It complements the workflow and Forge references; workflow authority still comes
from the compiled workflow and every mutation still goes through `Forge`.

## Scan cadence

`temper-worker` accepts one or more scan-shard repositories with `--repo
owner/name` or `--repo-list <path>`. The shard is not an authorization boundary:
the Forgejo token decides what the worker may read or mutate.

- `--poll-ms <n>` controls the active normal poll cadence. Role poll ticks scan
  the configured repository set using open-state queue candidate queries only;
  they do not list closed issues or closed/merged pull requests. Mechanical poll
  ticks run bounded reconciliation first, where workflow-labelled terminal
  recovery remains available, then run automated-queue scans for queues with
  `automation` metadata using the same open-state active candidate rule.
- `--idle-poll-max-ms <n>` caps the adaptive idle cadence for mechanical
  workers; the default cap is `60000` ms, raised to `--poll-ms` when the active
  poll interval is already longer. Consecutive successful normal mechanical
  ticks with `changed=false` and `actions=0` keep the first next poll at
  `--poll-ms`, then double the delay up to this cap. Any action/progress, wake
  tick, audit tick, or tick error resets the next normal poll to `--poll-ms`.
- `--audit-ms <n>` controls the low-frequency audit cadence. The default is
  `600000` ms; `0` disables audit ticks. Role audit ticks inspect all configured
  repositories and active/recoverable workflow-labelled recovery interest, but
  still avoid unlabelled closed history and pure identity-only terminal labels.
  Mechanical audit ticks are the explicit deep-audit path and may run
  all-history reconciliation.
- `--wake-socket <path>` plus optional `--wake-secret-file <path>` enables
  authenticated webhook wakeups for the legacy/internal wake-socket topology.
  Pull-request, review, CI/status, label-change, and push hints are all safe
  triggers for the same normal scan path. A wake with
  known repository hints immediately narrows role scans to the hinted configured
  repositories. No-hint or unknown hints fall back to a broad configured-repo
  scan. Mechanical wake scans still visit all configured repositories in
  production so cross-repo recovery can see dependency sources, but each per-repo
  reconciliation and automated-queue scan is bounded.
  `TEMPER_WAKE_DEBOUNCE_MS` can override the default 500ms local wake drain
  window when a deployment or fixture needs different burst coalescing. The
  worker binary reads it once at startup and passes a concrete duration to the
  wake bus (`temper-wake` itself never reads the environment).

Every tick re-reads fresh Forge state before planning or mutating. Engine or
standalone webhooks only accelerate latency; polling and audits remain the
correctness backstops.

## Agent liveness supervision

The deterministic cross-component and restart evidence is indexed in the
[agent-run liveness acceptance matrix](agent-run-liveness-acceptance.md).

`temper serve worker` owns one watchdog state per occupied permit. The effective
settings are visible in the generated configuration template and in
`temper config show`:

- `worker.max_no_progress_secs` (default 900) is reset only by accepted model,
  tool, steering, and terminal lifecycle boundaries; heartbeats do not count;
- optional `worker.max_run_secs` independently bounds the whole attempt;
- `worker.graceful_cancellation_grace_secs` and
  `worker.forced_termination_grace_secs` bound cooperative cancellation and
  escalation requests; a worker still fails closed and retains the attempt
  fence and permit while descendant inspection cannot prove emptiness;
- first-party operation limits live in `agent.deadlines` (with profile
  overrides) and must remain below the no-progress bound.

Use `temper config show` to verify resolved values without revealing provider or
worker credentials. During a run, inspect `GET /v1/state` (all workers) or
`GET /v1/state/job/<job-id>` (one assignment). The optional `worker_report`
shows attempt/phase, monotonic run and no-progress elapsed values, at most eight
content-free active model/tool summaries, timeout/cancellation status, and
pending-result state. It is a latest-report diagnostic, not lease or watchdog
authority.

On timeout the worker fences the attempt, requests escalation, and joins all
owned resources. It records a transient result and releases its local permit
only after direct-child reap, recursive descendant emptiness, and endpoint joins
are proven. A blocked inspection remains in `cleanup_pending`, retaining the
attempt fence and permit while throttled operator diagnostics continue. Result
delivery/replay and durable-claim convergence continue after a proven cleanup
and permit release. A Forge outage therefore appears as `worker.result.delivery`
retry warnings and eventually `assignment.convergence`, rather than
monopolizing capacity. Both graceful and forced termination append a synthetic
canonical terminal activity with `status=cancelled` even if the child cannot
send one.

## Process-containment capabilities and cleanup evidence

On Linux, the preferred backend uses a systemd-delegated cgroup-v2 subtree. A
job cgroup is prepared before agent spawn; nested tool, MCP, worker-command, and
pre-push cgroups are created below it and membership is established before
`exec`. The production contract requires a unified cgroup-v2 mount, writable
delegation and nested-subtree controls, and pidfd support. `cgroup.kill` is used
for hard escalation when available; otherwise Temper repeatedly enumerates and
signals every nested member. Direct-child reap and independent recursive-empty
verification gate completion in both cases.

When delegation or pidfd capability is unavailable, Linux activates its
subreaper/supervisor fallback. The fallback owns re-parented descendants and
tracks them independently of process groups and sessions. Windows uses nested,
kill-on-close Job Objects with assignment-before-resume and empty-job
verification. A host without one of those production backends fails
containment preparation rather than silently degrading to direct-child or
process-group-only cleanup.

The worker emits exactly one `worker.containment.startup_capability` diagnostic
per process. It reports the bounded cgroup-v2 mount identity, delegation,
nested-subtree writability, `cgroup.kill`, pidfd, selected backend, and fallback
reason. The same attempt-bound observer is installed in managed bash and MCP
containment for split-agent and standalone execution, so nested blocked,
fallback, and completed cleanup carries worker/job/attempt plus bounded
owner/tool identity instead of appearing only in the final job cleanup.
Unavailable delegation and `worker.containment.fallback_activated` are
warnings. `worker.containment.cleanup_completed` is debug for an ordinary
already-empty owner and warning when cleanup recovered leaked descendants or
inspection failures. `worker.containment.cleanup_blocked` is warning/error,
throttled by bounded containment root, and includes bounded survivor
PID/PPID/PGID/session/start-time/executable evidence without command arguments,
prompts, output, or credentials.

At startup, stale cgroups are considered owned only below Temper's dedicated
subtree. Their members are killed, trees that become empty are removed
deepest-first, and still-populated or uninspectable trees remain for retry.
`worker.containment.startup_scavenge` reports removed/retained counts, a bounded
list of retained path diagnostics, and the omitted-diagnostic count; retained or
omitted evidence is warning level. During `SIGINT` or `SIGTERM`, the worker
stops intake, fences all attempts, escalates every active owner, and joins the
active-job registry before returning. With the example systemd unit,
`Delegate=yes` permits nested
ownership and `KillMode=control-group` kills the complete service cgroup after
`TimeoutStopSec` if application cleanup cannot complete. Abrupt `SIGKILL`,
kernel failure, or power loss cannot produce a terminal cleanup event; the
service cgroup is the backstop and the next startup performs stale ownership
inspection.

## Diagnostics

Completed tick logs include:

```text
trigger=<initial|poll|wake|audit> actions=<n> tick_id=<id> \
scanned_repositories=<n> scanned_repository_paths=<owner/repo,...> \
next_poll_ms=<n> idle_no_action_ticks=<n>
```

Use these counters to verify hint narrowing (`wake` for repo A should list repo
A only for role workers) and to spot unexpected broad scans. Coordinated engine
runs additionally distinguish `wake.phase=start` / `wake.outcome=started` from
`wake.phase=finish` / `wake.outcome=completed`; failed finishes retain their
error and timing fields.

At worker debug level, every mechanical phase that starts emits one terminal
`measurement=mechanical.phase` record. Filter on `mechanical.phase` for
`reconciliation`, `automated_scan`, or `transition_application`, then compare
numeric `duration_ms` and `provider.request_total`. `mechanical.scope` is
`broad` or `targeted`, exact work includes `artifact.ref`, and all records inherit
the enclosing `wake.run_id`. `provider.requests_available=false` means the
backend has no portable request counter; use Forge HTTP `operation` records with
the same wake id instead.

`gate.evaluated` is also debug: repeated lines are repeatable read-side
observations, not merge execution. Actual direct merge execution is paired as
`measurement=mechanical.landing_attempt`: `landing.outcome=started` followed by
a duration-bearing terminal `applied`, `gate_not_satisfied`,
`conflict_routed`, `stale`, or `failed`. If an eligible gate appears without an
attempt, confirm the compiled transition contains `merge_pull_request`; non-merge
automation intentionally emits no attempt.

Mechanical workers also emit `mechanical_reconciliation_summary` with `mode`,
`snapshot_count`, `finding_count`, and applied/advisory action counts. Normal
mechanical ticks emit `mechanical_automation_execution` per automated item and
`mechanical_automation_summary` with candidate, applied, unchanged,
gate-not-satisfied, and error counts. When `TEMPER_FORGEJO_CI_DIAGNOSTICS` is set
to a non-blank value, Forgejo web-UI CI fallback reads are logged as
`read_ci_jobs_via_web_ui`; non-CI role ticks should not produce them. The worker
binary reads this env var at startup and sets the backend's `ci_diagnostics`
config flag explicitly (the backend never reads the environment).
