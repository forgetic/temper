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

`temper serve worker` owns one watchdog state per occupied permit. The effective
settings are visible in the generated configuration template and in
`temper config show`:

- `worker.max_no_progress_secs` (default 900) is reset only by accepted model,
  tool, steering, and terminal lifecycle boundaries; heartbeats do not count;
- optional `worker.max_run_secs` independently bounds the whole attempt;
- `worker.graceful_cancellation_grace_secs` and
  `worker.forced_termination_grace_secs` bound cooperative cancellation,
  process-group escalation, descendant cleanup, and join;
- first-party operation limits live in `agent.deadlines` (with profile
  overrides) and must remain below the no-progress bound.

Use `temper config show` to verify resolved values without revealing provider or
worker credentials. During a run, inspect `GET /v1/state` (all workers) or
`GET /v1/state/job/<job-id>` (one assignment). The optional `worker_report`
shows attempt/phase, monotonic run and no-progress elapsed values, at most eight
content-free active model/tool summaries, timeout/cancellation status, and
pending-result state. It is a latest-report diagnostic, not lease or watchdog
authority.

On timeout the worker fences the attempt, cancels and joins all owned resources,
durably records one transient result, releases its local permit, and polls
immediately. Result delivery/replay and durable-claim convergence continue after
permit release. A Forge outage therefore appears as
`worker.result.delivery` retry warnings and eventually
`assignment.convergence`, rather than monopolizing capacity. Both graceful and
forced termination append a synthetic canonical terminal activity with
`status=cancelled` even if the child cannot send one.

## Diagnostics

Completed tick logs include:

```text
trigger=<initial|poll|wake|audit> actions=<n> tick_id=<id> \
scanned_repositories=<n> scanned_repository_paths=<owner/repo,...> \
next_poll_ms=<n> idle_no_action_ticks=<n>
```

Use these counters to verify hint narrowing (`wake` for repo A should list repo
A only for role workers) and to spot unexpected broad scans. Mechanical workers
also emit `mechanical_reconciliation_summary` with `mode`, `snapshot_count`,
`finding_count`, and applied/advisory action counts. Normal mechanical ticks emit
`mechanical_automation_execution` per automated item and
`mechanical_automation_summary` with candidate, applied, unchanged,
gate-not-satisfied, and error counts. When `TEMPER_FORGEJO_CI_DIAGNOSTICS` is set
to a non-blank value, Forgejo web-UI CI fallback reads are logged as
`read_ci_jobs_via_web_ui`; non-CI role ticks should not produce them. The worker
binary reads this env var at startup and sets the backend's `ci_diagnostics`
config flag explicitly (the backend never reads the environment).
