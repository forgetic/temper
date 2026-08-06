# Recover a codebase-memory cache safely

Use this runbook to review and reclaim obsolete Temper-owned provider projects,
then rebuild and verify one configured logical repository. The supported path is
`temper maintenance codebase-memory`; **do not delete the provider cache
directory**.

The command uses the deployment's resolved `[agent.tools.codebase_memory]`
command, arguments, timeouts, workspace layout, ownership classifier, and
retention bounds. It is host-only. Provider deletion and indexing are not
registered as coding-agent/model tools.

## 1. Verify the deployment and provider

Use the same config and secrets selection as the affected worker deployment.
Global options must precede the command:

```sh
temper --config /etc/temper/config.toml \
  --secrets /run/credentials/temper \
  maintenance codebase-memory --help
```

Confirm that:

- the configured provider is `codebase-memory-mcp` 0.9.0 or newer;
- `[agent.tools.codebase_memory.retention]` is enabled and its age, count,
  inventory-page, and deletion limits are appropriate;
- `[paths]`/worker workspace points at the affected Temper workspace root; and
- the selected logical repository (for example `ai/temper`) occurs in
  `[engine] repos`.

The command negotiates the provider name/version and required bounded inventory
and maintenance schemas. It also verifies one stable project's targeted status,
stable index upsert, and safe probe schemas when those operations are requested.
A provider or cache instance that cannot be identified causes a deletion-free
failure.

## 2. Stop or quiesce every worker

Recovery must not race assignments or indexing. Stop admission, wait for active
jobs and background indexes to finish, and then stop every process that can
launch a codebase-memory provider:

- **standalone:** stop the `temper serve standalone` service;
- **split deployment:** stop every `temper serve worker`/`temper-worker` pool,
  not only the engine; and
- stop manually launched coding-agent or indexing processes using this cache.

Follow the deployment's normal service manager procedure (for example,
`systemctl stop ...`) and verify the processes are gone. The recovery command
also takes the same workspace maintenance lock as periodic retention and refuses
inventory that reports active indexing. A lock refusal or active-indexing report
means quiescence is incomplete; do not bypass it.

## 3. Review a dry-run

Dry-run is the default and never calls provider deletion:

```sh
temper --config /etc/temper/config.toml \
  --secrets /run/credentials/temper \
  maintenance codebase-memory --repository ai/temper
```

For machine review, put the global format option before the command:

```sh
temper --config /etc/temper/config.toml \
  --secrets /run/credentials/temper \
  --format json \
  maintenance codebase-memory --repository ai/temper > /secure/tmp/cbm-plan.json
```

Review all reported evidence:

- verified provider name/version and cache instance identity;
- complete inventory record count and cache bytes, or the explicit
  `unavailable`/`null` marker when the provider cannot estimate bytes;
- configured age, count, page, and per-run deletion bounds;
- `plan_id`, which binds a later apply to this exact verified classification,
  provider version, cache instance, and selected stable logical identity;
- candidate provider identities, paths, byte estimates, reasons, and exact
  proposed actions; and
- preserved records and reasons.

The output is bounded to 100 records per disposition class and reports omitted
counts. If records are omitted, narrow the configured inventory/retention scope
or archive the JSON in a protected location and review in repeatable batches;
do not replace this with raw cache-directory inspection/deletion.

Expected preserves include the stable `temper-v1-*` projects, existing/active
workspaces, records outside the canonical workspace root, unrelated projects,
and records with ambiguous ownership or path ancestry. If any intended preserve
appears as a candidate, stop and correct configuration or provider metadata.

## 4. Apply only confirmed candidates

After the dry-run is understood and workers remain stopped, copy its top-level
`plan_id` and add both the explicit apply flag and plan binding:

```sh
PLAN_ID=$(jq -r .plan_id /secure/tmp/cbm-plan.json)
temper --config /etc/temper/config.toml \
  --secrets /run/credentials/temper \
  --format json \
  maintenance codebase-memory --apply --plan "$PLAN_ID" \
  --repository ai/temper > /secure/tmp/cbm-apply.json
```

Apply first requires the supplied `--plan` to match a fresh complete dry-run
for the same selected logical repository, then validates the recovery tool
schemas and explicit rebuild source before mutation and performs a second
inventory preflight under one maintenance lock. Provider/cache identity,
candidate and preserve classes, paths, byte estimates, and proposed actions must
match exactly. Every proposed identity must also return a known, non-active
targeted `index_status`; missing, changed, actively indexing, or unverifiable
candidates fail closed. Deletion is then limited to the exact verified
`proposed` identities and the configured per-run cap. Individual provider
failures remain visible and retryable; unrelated, active, stable, and ambiguous
records are not deleted.

Rerun dry-run after each bounded apply. Empty `proposed` and `deleted` lists are
an idempotent completion, not a reason to delete files manually.

## 5. Rebuild the stable logical project

Prepare an explicit, trusted checkout for the configured repository. Do not rely
on the shell's current directory or a coordination-scoped worker checkout. Then
request a host-controlled stable upsert:

```sh
temper --config /etc/temper/config.toml \
  --secrets /run/credentials/temper \
  --format json \
  maintenance codebase-memory --apply --plan "$PLAN_ID" \
  --repository ai/temper \
  --rebuild-from /srv/data/git/recovery/temper
```

`--rebuild-from` requires `--repository`, `--apply`, and the `--plan` from a
fresh reviewed dry-run. Temper canonicalizes and validates the explicit source directory before any
deletion, but derives the provider project key solely from the configured
logical Forge identity (`forgejo:ai/temper` plus owner/name), the same stable-key
algorithm used at runtime. The checkout path never becomes the project
identity.

The command waits for `index_repository`, performs targeted `index_status` on
the stable key, and calls the read-only `search_code` safe probe. It emits no
probe content, only success/readiness evidence and targeted lookup latency.

## 6. Retry and rollback guidance

- On timeout, changed preflight, active indexing, incomplete discovery, unknown
  cache identity, or ambiguous metadata: leave workers stopped, fix that cause,
  and repeat dry-run. Never increase bounds merely to suppress uncertainty.
- On an isolated deletion failure: retain the apply report, rerun dry-run, and
  retry the still-proposed identity. Provider "not found" is treated
  idempotently.
- Deletion has no byte-for-byte rollback. Recovery is rebuilding the affected
  stable logical project from the trusted source checkout. Unrelated/stable
  records are the rollback boundary and must remain protected.
- If stable rebuild or probe fails, do not resume workers. Correct the provider
  or source checkout and rerun the targeted rebuild. Do not remove the whole
  provider cache.

## 7. Post-recovery checks

Before restarting workers, retain a final JSON report and verify:

1. **Project count:** `retention.inventory_record_count` is at the expected
   bounded level and repeat dry-runs propose no unintended deletion.
2. **Cache bytes:** `retention.cache_bytes` decreased as expected, or is clearly
   `null` when the provider does not expose byte data.
3. **Targeted lookup latency:** `stable_project.lookup_latency_ms` is recorded
   and acceptable for the deployment timeout.
4. **Stable identity:** `stable_project.provider_key` is `temper-v1-*` and is
   unchanged across source checkout paths.
5. **Index readiness:** `stable_project.ready` is true with a ready/fresh/indexed
   status.
6. **Safe tool probe:** `stable_project.safe_probe_succeeded` is true.
7. **Isolation:** preserved active, unrelated, stable, and ambiguous records
   remain present with their preserve reasons.

Finally restart worker pools (and standalone/engine services as applicable),
watch the first targeted discovery/index-readiness events, and run one more
status-only dry-run with `--repository ai/temper`.
