# Agent trace query API

The engine exposes finite, one-shot reads over its durable agent trace journal.
These routes are enabled only when both the engine journal and the named
`observability.agent_traces.read_token` secret value are available.

## Authorization

Every request below requires:

```http
Authorization: Bearer <configured read token>
```

A missing header returns `401`, and a malformed or incorrect credential returns
`403`. When no read token or journal is available, the whole surface behaves as
not found (`404`). Run lookup happens only after authorization, and `404`
responses do not disclose whether a run exists. Trace responses use
`Cache-Control: no-store`.

`GET /v1/state` and `GET /v1/state/job/{id}` remain unauthenticated and do not
contain trace manifests, captured messages, source, or tool content.

## Routes

### List runs

```http
GET /v1/agent-runs?artifact_ref=...&role=...&correlation_key=...
```

Exact-match filters are `artifact_ref`, `role`, `correlation_key`,
`agent_session_id`, `status`, and `run_id`. Status is one of `active`,
`succeeded`, `cancelled`, or `failed`. All supplied filters compose with AND.
Unknown or duplicate parameters return `400`.

Pages are in ascending `(started_at, run_id)` order. `run_id` is the tie breaker
when timestamps are equal, so order is stable across journal restarts. The
response's opaque `next_cursor` is passed back as `cursor`; it is bound to the
active filters and cannot be reused with a different filter set. `limit`
defaults to 50 and must be between 1 and 200.

```json
{
  "runs": [
    {
      "version": 1,
      "run_id": "run-a",
      "identity": {
        "worker_id": "worker-a",
        "assignment_id": "assignment-a",
        "job_id": "job-a",
        "repository": "ai/temper",
        "artifact_ref": "ai/temper#311",
        "role": "engineer",
        "action": "open_pr",
        "correlation_key": "pr-for-code-311",
        "agent_session_id": "session-a"
      },
      "status": "succeeded",
      "started_at": "2099-01-01T00:00:00Z",
      "completed_at": "2099-01-01T00:00:01Z",
      "duration_ms": 1000,
      "counts": {
        "events": 2,
        "scopes": 0,
        "turns": 0,
        "model_calls": 0,
        "tool_calls": 0,
        "retries": 0
      },
      "usage": {
        "input_tokens": 0,
        "output_tokens": 0,
        "cache_read_tokens": 0,
        "cache_write_tokens": 0
      },
      "capture_mode": "transcript",
      "has_truncated_content": false,
      "has_trace_gaps": false,
      "dropped_events": 0,
      "first_seq": 1,
      "last_seq": 2
    }
  ],
  "next_cursor": "opaque"
}
```

### Read one summary

```http
GET /v1/agent-runs/{percent-encoded-run-id}
```

The response is the same typed summary used in a list page. Partial runs have
`status: "active"` and remain readable.

### Read events

```http
GET /v1/agent-runs/{percent-encoded-run-id}/events?after_seq=42&limit=500
```

Events are strictly ascending by canonical run sequence and include only events
with `seq > after_seq`. `after_seq` defaults to 0. `limit` defaults to 500 and
must be between 1 and 1000. `next_after_seq` is the last returned sequence (or
the supplied cursor for an empty page), allowing polling to resume when new
events are appended.

```json
{
  "run_id": "run-a",
  "events": [],
  "next_after_seq": 42,
  "has_more": false
}
```

Authorized responses retain bounded inline content and blob references allowed
by capture policy. Before a response is built, the journal revalidates the run
path, regular-file constraints, sequence stream, blob paths, sizes, and SHA-256
digests.

### Export JSONL

```http
GET /v1/agent-runs/{percent-encoded-run-id}/export
```

The finite `application/x-ndjson` body contains one canonical event per line in
strictly ascending sequence order. Export takes no query parameters.

## Errors

Malformed percent encoding, cursors, numbers, limits, duplicate parameters, and
unknown query parameters return a JSON `400` response. Authorized missing runs
return the same generic JSON `404` shape used while the route surface is
disabled. Journal read or integrity failures return a generic `500` without
filesystem details, captured content, or credentials.
