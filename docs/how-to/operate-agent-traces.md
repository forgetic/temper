# Operate durable agent traces and OpenTelemetry

Temper records agent activity as canonical, append-only events first. The web
run drawer, JSONL export, operational tracing, and OpenTelemetry are projections
of those durable events; OpenTelemetry is not transcript storage.

## Configure capture and read access

Set a durable `paths.state_dir`. Trace roots are derived from it and must remain
outside `paths.workspace_dir`:

```toml
[observability.agent_traces]
capture = "metadata"       # off | metadata | transcript | diagnostic
retention_days = 14
max_run_bytes = 50000000
capture_thinking = false
read_token = "agent-trace-reader"
```

Put the token value in the credentials file, never in `temper.toml`:

```toml
[secrets]
agent-trace-reader = "replace-with-a-long-random-value"
```

`metadata` is the safe default. It records identities, boundary names, timing,
status, model/provider labels, usage, retries, and gaps. It excludes prompts,
assistant text, thinking, tool arguments/results, credentials, headers, and
environment values. `transcript` permits bounded sanitized visible content.
`diagnostic` additionally permits bounded deltas; thinking still requires
`capture_thinking = true`. Do not use diagnostic capture as a routine production
setting.

Run `temper check` after changing capture policy. If no durable state directory
can be resolved, trace storage is disabled rather than failing assigned work.

## Storage, permissions, quota, and retention

The resolved state directory contains:

```text
$STATE_DIR/agent-traces/
  journal/runs/<sha256-of-run-id>/
    manifest.json
    events.jsonl
    summary.json
    source-digests.jsonl
    blobs/
  worker-spool/<run-id>/
    manifest.json
    events.jsonl                 # truncated after terminal acknowledgement
    acknowledgement.json
    terminalization.json         # restart-safe synthetic-terminal intent, when used
    compacted.json               # durable terminal acknowledgement marker
    blobs/                       # emptied after terminal acknowledgement
  worker-spool/quarantine/<original-name>.bad/
    ...                          # preserved malformed spool bytes
```

Directories are created owner-only (`0700`) and files owner-only (`0600`) on
Unix. Keep the state directory on persistent local storage, back it up under the
same access controls as source code, and do not place it below a workstream
checkout. Workstream cleanup intentionally does not remove trace history.

`max_run_bytes` bounds each run. The worker reserves each growable,
non-terminal run's complete budget against an aggregate spool ceiling of 16 ×
`max_run_bytes`. Once recovery or the original owner makes a run durably
terminal, that immutable run is charged its actual spool bytes while it awaits
engine acknowledgement; a compacted acknowledgement marker likewise counts only
its actual bytes. This physical accounting is what releases abandoned logical
reservations without deleting their payload. Quarantined bytes are reported as
physical usage but do not reserve active-spool quota.

When that aggregate ceiling is exhausted, new runs continue without durable
tracing and emit an operator warning—the assigned job result and retry policy do
not change. Partial acknowledgements only advance the durable cursor and retain
the restart-readable payload. Once the engine acknowledges a terminal run's
highest contiguous sequence, the worker atomically installs `compacted.json`
before truncating `events.jsonl` and removing its blobs. That marker lets a
terminal flush or restarted forwarder observe the durable acknowledgement
without retaining the transcript indefinitely. Valid evidence that has not yet
been forwarded is never deleted by capacity recovery.

Optional content is omitted before required
run/scope/turn/model/tool/usage/error/terminal boundaries. Host-generated
`run.failed` events never copy provider/tool errors or child stderr: they carry
only the trusted failure code, retryability, and a fixed allowlisted summary.
The bounded stderr tail remains available in worker diagnostics and job failure
reporting, outside spools, journals, queries, exports, web activity, and OTel.
Queue pressure drops
or coalesces only low-priority deltas and journals `trace.gap` counts. Engine
retention runs at startup and every hour in both split and standalone services;
each pass snapshots live daemon assignments, skips active and recovered
in-flight runs, isolates per-run cleanup failures, and continues on the next
cadence. Shutdown cancels and joins the retention component before releasing
assignments. A partial final JSONL fragment is truncated to the last complete
record during recovery, while the readable partial run stays queryable.

## Understand restart reclamation

Split workers and the in-process standalone worker use the same startup path.
Before assignment registration or polling, it deterministically inspects up to
16 actionable spool entries. This bound is large enough to recover one fully
reserved 16-run ceiling. If dirty work remains, bounded background passes yield
between scans so product work is not held indefinitely.

Recovery takes a non-blocking lifetime-ownership lock for each run. A run still
owned by a live attempt is reported as `protected` and is not opened, truncated,
or terminalized. After that ownership ends, a later bounded pass may reclaim it.
For a valid abandoned stream, recovery preserves every complete event and
referenced blob, truncates only an incomplete final JSONL fragment, and appends
one fixed, privacy-safe synthetic `run.failed` boundary. The durable
`terminalization.json` intent makes this exactly-once across restart boundaries.
The ordinary at-least-once forwarder then sends the complete stream to the
engine journal; duplicate delivery is safe by `(run_id, seq)`. Local event/blob
payload is compacted only after the engine's durable terminal acknowledgement.

Malformed manifests, complete malformed event records, invalid cursors, and
unsafe layouts are atomically moved to
`$STATE_DIR/agent-traces/worker-spool/quarantine/`. Collision-safe `.bad` names
preserve every byte and owner-only permissions. One malformed sibling therefore
cannot reserve active-spool capacity or block healthy runs. Quarantine is not an
automatic deletion area: inspect a copy under the same privacy controls, retain
it according to incident policy, and stop the worker before manually archiving
or removing the original.

Each startup pass emits `agent.activity.startup_recovery` with both structured
fields and an ordinary human summary. For example:

```text
worker startup activity recovery: terminalized 16, quarantined 0, protected 0, failed 0, remaining dirty 1, physical used bytes 142000000, logical reserved bytes 142000000
```

The fields are `terminalized_runs`, `quarantined_runs`, `protected_runs`,
`failed_runs`, `remaining_dirty_runs`, `physical_used_bytes`, and
`logical_reserved_bytes`. A nonzero `failed` or `remaining dirty` count means
background recovery will retry while assignments remain fail-open.

## Query and export JSONL

Every trace route requires the configured bearer token. Without a resolved token
the routes remain disabled. For example:

```sh
TOKEN='replace-with-a-long-random-value'
curl -H "Authorization: Bearer $TOKEN" \
  'http://127.0.0.1:8080/v1/agent-runs?artifact_ref=ai%2Ftemper%23313'

curl -H "Authorization: Bearer $TOKEN" \
  'http://127.0.0.1:8080/v1/agent-runs/RUN_ID/events?after_seq=0&limit=500'

curl -H "Authorization: Bearer $TOKEN" \
  'http://127.0.0.1:8080/v1/agent-runs/RUN_ID/export' > run.jsonl
```

See [Agent trace query API](../reference/agent-trace-query-api.md) for exact
pagination, filtering, status, and error contracts. The web service keeps the
read token server-side, polls these finite endpoints by sequence cursor, and
fans details out only while a drawer is open. Browser reconnect uses the last
sequence and therefore does not make the web ring authoritative.

## Enable OpenTelemetry

OTel is disabled by default. Build the unified binary or a slim service with the
OTLP feature forwarding enabled:

```sh
cargo build --release --features otel
cargo build --release -p temper-engine-service --features otel
```

The exporter uses OTLP/HTTP protobuf and the standard upstream variables. A
local collector example is:

```sh
docker run --rm -p 4318:4318 otel/opentelemetry-collector:latest
export OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318
export OTEL_SERVICE_NAME=temper-engine
./target/release/temper --config /etc/temper/temper.toml serve standalone
```

For distributed deployment, enable the feature on each binary that should emit
operational spans. The engine projects canonical durable activity after ingest,
so run/scope/turn/model/tool span trees are the same whether collection began in
an in-process standalone worker or a remote worker. Collector headers and
endpoints configure the exporter only and are never copied to span attributes.

Canonical spans include event timestamps, duration, status, provider/model,
usage/cache tokens, retry delay/count, and gap counts. They structurally cannot
carry transcript or tool bodies. Optional W3C `traceparent`/`tracestate` follows
one assignment through `Assign`, `JobContext`, `WorkspaceContext`, and the
worker-stamped run identity. Each later workstream run remains a distinct run
root; `correlation.key` and `agent.session.id` provide durable linkage instead
of a multi-day parent span.

The `otel` feature without `otel-otlp` is useful to embedding applications that
install their own exporter while reusing the layer and canonical projector:

```sh
cargo build -p temper-log --features otel
```

Tests use `InMemoryActivitySpanExporter`, which preserves deterministic finish
order and requires no network or collector.

## Troubleshoot

- **No runs:** verify `paths.state_dir`, capture is not `off`, and inspect startup
  warnings about journal/spool creation. Run `temper config show` to confirm the
  derived roots (secret values remain redacted).
- **Queries return 404:** configure a resolvable named `read_token`; the route is
  intentionally hidden when disabled. `401` means missing authorization and
  `403` means malformed or wrong credentials.
- **Trace capacity warning:** the ordinary line includes all admission values,
  even when the log formatter omits the structured error. For example:
  `standalone worker could not start durable agent tracing; continuing without
  it (physical used bytes 137000000, logical reserved bytes 750000000,
  requested bytes 50000000, limit 800000000, dirty runs 15)`. Compare logical
  reservation with physical use, then inspect the latest
  `agent.activity.startup_recovery` summary. Restart the worker to run the
  startup pass; do not delete active spool directories to make room.
- **Runs stay active:** inspect worker spool acknowledgement and engine logs. A
  child crash should end with a host-generated failure; a worker/engine outage
  leaves events queued for at-least-once retransmission. Duplicate batches are
  deduplicated by `(run_id, seq)`. A `protected` count means an owner fence is
  still live; verify the assignment before stopping that owner, then let a later
  bounded pass reclaim it. A `quarantined` count points to preserved evidence
  under `worker-spool/quarantine`; inspect a copy rather than moving bytes back
  into the active spool.
- **No OTel spans:** confirm the binary was built with `otel`, port 4318 is
  reachable, and `OTEL_EXPORTER_OTLP_ENDPOINT` uses HTTP. The exporter falls
  back to a no-op provider if initialization fails.
- **Collector outage or full trace disk:** fix telemetry independently. Socket,
  spool, journal, projection, exporter, admission, and reclamation failures are
  deliberately unable to change job success/failure, retry policy, or any other
  product outcome. Preserve valid unforwarded spool and quarantine evidence;
  tracing failure is not authority to delete it or to retry product work.
