# Codebase-memory lifecycle observability

Status: stable · Audience: operators and alert authors

Temper emits content-free lifecycle evidence from targeted identity discovery,
index decisions, readiness waits, and bounded maintenance reports. Runtime logs
do not contain checkout roots, command arguments, provider records, source text,
credentials, or candidate details. Candidate records remain available only in an
explicit operator response.

## Event catalog

- `codebase_memory.discovery.completed` is debug on success and warn otherwise.
  It reports targeted startup discovery and carries `discovery.method`,
  `discovery.inventory`, `discovery.targeted`, `duration_ms`, `outcome`,
  `timed_out`, `record_count`, `cache.bytes_available`, `cache.bytes`, and
  `failure.category`.
- `codebase_memory.maintenance.discovery.completed` reports the same fields for
  worker-owned bounded maintenance inventory at the same levels.
- `codebase_memory.identity.selected` is debug. It carries bounded
  `identity.logical`, stable `identity.provider`, and `identity.outcome`:
  `reused`, `migrated`, `missing`, or `stale`.
- `codebase_memory.index.lifecycle` is debug except for warn-level failures. It
  carries both identities, `index.mode`, `index.outcome`, and
  `failure.category`.
- `codebase_memory.readiness.wait` is debug on success and warn otherwise. It
  carries stable provider identity, `duration_ms`, `outcome`, `timed_out`, and
  `failure.category`.
- `codebase_memory.retention.completed` is info, or warn when partial,
  uncertain, or timed out. It carries policy fields, `duration_ms`, aggregate
  counts, estimated deleted bytes, `retention.dry_run`, `failure.count`, and
  `outcome`.

Index outcomes are `requested`, `started`, `reused`,
`suppressed_duplicate`, `completed`, `failed`,
`skipped_discovery_unknown`, and `disabled`. A discovery timeout therefore has
both `outcome=timeout` and a matching
`index.outcome=skipped_discovery_unknown`; it is never reported as confirmed
missing.

`cache.bytes=0` is meaningful only when `cache.bytes_available=true`. The same
rule applies to `retention.deleted_estimated_bytes` and
`retention.deleted_bytes_available`. Providers that cannot report bytes produce
an explicit false availability field, not an inferred zero-byte cache.

Failure messages are closed, bounded summaries. Use `failure.category` for
automation. Provider diagnostics and per-record deletion failures remain in the
explicit maintenance result rather than normal runtime logs.

## Queries and alerts

Under systemd, use `journalctl -o json`; for the explicit JSON stderr sink,
replace the command prefix with the JSON log file producer.

```sh
# Discovery latency and timeouts. Alert on timeouts or sustained p95 growth.
journalctl -u temper -o json | jq -c \
  'select((.event|IN("codebase_memory.discovery.completed",
                     "codebase_memory.maintenance.discovery.completed"))) |
   {inventory:."discovery.inventory",outcome,timed_out,duration_ms,
    records:.record_count,
    bytes:(if ."cache.bytes_available"
           then ."cache.bytes" else null end)}'

# Provider project count and cache growth. Graph by inventory kind.
journalctl -u temper -o json | jq -c \
  'select((.event|IN("codebase_memory.discovery.completed",
                     "codebase_memory.maintenance.discovery.completed")) and
          .outcome=="success") |
   {inventory:."discovery.inventory",projects:.record_count,
    cache_bytes:(if ."cache.bytes_available"
                 then ."cache.bytes" else null end)}'

# Duplicate requests. Alert if these recur for one provider identity.
journalctl -u temper -o json | jq -c \
  'select(.event=="codebase_memory.index.lifecycle" and
          ."index.outcome"=="suppressed_duplicate") |
   {provider:."identity.provider",mode:."index.mode"}'

# Readiness failures and timeouts. Alert on every returned record.
journalctl -u temper -o json | jq -c \
  'select(.event=="codebase_memory.readiness.wait" and
          .outcome!="success") |
   {provider:."identity.provider",outcome,duration_ms,
    category:."failure.category"}'

# Retention failures, uncertainty, and deletion volume.
journalctl -u temper -o json | jq -c \
  'select(.event=="codebase_memory.retention.completed" and
          (."failure.count">0 or
           (.outcome|IN("timed_out","discovery_failed",
                        "inventory_uncertain")))) |
   {outcome,candidates:."retention.candidate_count",
    deleted:."retention.deletion_count",failures:."failure.count",
    dry_run:."retention.dry_run"}'
```

Recommended signals are: any discovery or readiness timeout; repeated duplicate
suppression; monotonic project-count or cache-byte growth without successful
retention; any `partial_failure`; and consecutive `discovery_failed`,
`inventory_uncertain`, or `timed_out` retention outcomes. A disabled or
active-work-suppressed pass is state evidence, not a cleanup failure.
