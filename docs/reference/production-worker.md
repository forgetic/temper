# Production worker runtime

This page records the operator-visible knobs on the Forgejo `temper-worker` binary.
It complements the workflow and Forge references; workflow authority still comes
from the compiled workflow and every mutation still goes through `Forge`.

## Scan cadence

`temper-worker` accepts one or more scan-shard repositories with `--repo
owner/name` or `--repo-list <path>`. The shard is not an authorization boundary:
the Forgejo token decides what the worker may read or mutate.

- `--poll-ms <n>` controls the normal poll cadence. Poll ticks scan the
  configured repository set using normal bounded candidate queries.
- `--audit-ms <n>` controls the low-frequency audit cadence. The default is
  `600000` ms; `0` disables audit ticks. Role audit ticks inspect all configured
  repositories and all workflow-labelled recovery interest, but still avoid
  unlabelled closed history. Mechanical audit ticks are the explicit deep-audit
  path and may run all-history reconciliation.
- `--wake-socket <path>` plus optional `--wake-secret-file <path>` enables
  authenticated webhook wakeups. A wake with known repository hints immediately
  narrows role scans to the hinted configured repositories. No-hint or unknown
  hints fall back to a broad configured-repo scan. Mechanical wake scans still
  visit all configured repositories in production so cross-repo recovery can see
  dependency sources, but each per-repo reconciliation is bounded.
  `TEMPER_WAKE_DEBOUNCE_MS` can override the default 500ms local wake drain
  window when a deployment or fixture needs different burst coalescing.

Every tick re-reads fresh Forge state before planning or mutating. Webhooks only
accelerate latency; polling and audits remain the correctness backstops.

## Diagnostics

Completed tick logs include:

```text
trigger=<initial|poll|wake|audit> tick_id=<id> actions=<n> \
scanned_repositories=<n> scanned_repository_paths=<owner/repo,...>
```

Use these counters to verify hint narrowing (`wake` for repo A should list repo
A only for role workers) and to spot unexpected broad scans. Mechanical workers
also emit `mechanical_reconciliation_summary` with `mode`, `snapshot_count`,
`finding_count`, and applied/advisory action counts. When
`TEMPER_FORGEJO_CI_DIAGNOSTICS=1` is set, Forgejo web-UI CI fallback reads are
logged as `read_ci_jobs_via_web_ui`; non-CI role ticks should not produce them.
