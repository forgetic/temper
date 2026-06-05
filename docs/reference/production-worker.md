# Production worker runtime

This page records the operator-visible knobs on the Forgejo `temper-worker` binary.
It complements the workflow and Forge references; workflow authority still comes
from the compiled workflow and every mutation still goes through `Forge`.

## Scan cadence

`temper-worker` accepts one or more scan-shard repositories with `--repo
owner/name` or `--repo-list <path>`. The shard is not an authorization boundary:
the Forgejo token decides what the worker may read or mutate.

- `--poll-ms <n>` controls the active normal poll cadence. Poll ticks scan the
  configured repository set using normal bounded candidate queries. Terminal
  closed/merged history is queried only through active or recoverable labels
  (queue labels, state labels, transition labels, and gate/condition labels),
  not through pure artifact-kind identity labels such as a completed PR's
  `implementation` label. Mechanical poll ticks run bounded reconciliation
  first, then bounded automated-queue scans for queues with `automation`
  metadata.
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
  authenticated webhook wakeups. Pull-request, review, CI/status, label-change,
  and push hints are all safe triggers for the same normal scan path. A wake with
  known repository hints immediately narrows role scans to the hinted configured
  repositories. No-hint or unknown hints fall back to a broad configured-repo
  scan. Mechanical wake scans still visit all configured repositories in
  production so cross-repo recovery can see dependency sources, but each per-repo
  reconciliation and automated-queue scan is bounded.
  `TEMPER_WAKE_DEBOUNCE_MS` can override the default 500ms local wake drain
  window when a deployment or fixture needs different burst coalescing.

Every tick re-reads fresh Forge state before planning or mutating. Webhooks only
accelerate latency; polling and audits remain the correctness backstops.

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
gate-not-satisfied, and error counts. When `TEMPER_FORGEJO_CI_DIAGNOSTICS=1` is
set, Forgejo web-UI CI fallback reads are logged as `read_ci_jobs_via_web_ui`;
non-CI role ticks should not produce them.
