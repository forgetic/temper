# Worker/Agent process protocol v1

The worker/agent protocol is the per-assignment boundary between
`temper-worker` and an out-of-process coding agent. The standalone deployment
uses the same DTOs in process; only the carrier changes. The serde-only contract
lives in `temper-protocol-agent`.

## Invocation and lifetime

The worker starts one agent in the prepared workspace and supplies non-secret
inputs as flags:

- `--context <FILE>`: one worker-written `WorkspaceContext` JSON document;
- `--result <FILE>`: the terminal `WorkspaceResult` JSON destination;
- `--workspace <DIR>`: workspace root and process cwd;
- optional `--submit-for-pr-address <HOST:PORT>` and
  `--forge-context-address <HOST:PORT>` loopback side channels;
- optional `--tool-config <FILE>` for non-secret agent-local tools;
- `--runtime-limits <FILE>` for known first-party commands only. The file is an
  `AgentRuntimeLimitsV1` JSON object with positive `tool_timeout_secs`,
  `model_connect_timeout_secs`, and `model_idle_timeout_secs` values.

The worker classifies each invocation as first-party or third-party. Known
`temper-agent` and `temper agent` commands receive complete resolved limits and
first-party lifecycle flags. Explicit third-party profile commands receive no
Temper-specific limits flag and remain under bounded worker fallback
supervision. Profile deadline overrides inherit field-by-field from
`[agent.deadlines]` before the file is written.

The side-channel listeners exist only for that run, accept bounded JSON, and
are stopped when the child exits. Standalone calls the same host callbacks
in-process rather than opening sockets.

The worker writes per-run protocol files in a private temporary directory. Its
durable result root is created with owner-only permissions below
`[paths].state_dir` when available, or below the worker workspace root otherwise.

## `WorkspaceContext` and artifact compatibility

`WorkspaceContext.artifact_context` is an optional versioned
`ArtifactContextBundle`. It contains the primary artifact, mandatory ancestry,
compact related indexes, directed relations, diagnostics, and explicit
truncation flags. The agent renders the bundle into stable sections (primary,
lineage, validation summaries, optional references, and diagnostics) instead of
asking a model to interpret raw graph JSON.

The historical singular carrier is still present at
`WorkspaceContext.work_item.context`. It remains the original inner work-item
JSON string, including the legacy `artifact` snapshot. Producers copy both
carriers without reconstructing either one. Agents predating bundles ignore
`artifact_context`; current agents fall back to rendering
`work_item.context` under `Work item context (JSON)` when the bundle is absent.

Abbreviated current shape:

```json
{
  "trace_context": {
    "traceparent": "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
    "tracestate": "vendor=opaque"
  },
  "work_item": {
    "role": "engineer",
    "queue": "code_ready",
    "kind": "code",
    "target": "Issue { number: ItemNumber(285) }",
    "context": "{\"artifact\":{\"number\":285,\"title\":\"Verify lineage\"}}"
  },
  "artifact_context": {
    "version": 1,
    "repository": {"id": "repo-1", "path": "ai/temper"},
    "artifact_type": "issue",
    "primary": {
      "artifact": {
        "repository": {"id": "repo-1", "path": "ai/temper"},
        "artifact_type": "issue",
        "number": 285
      },
      "title": "Verify lineage",
      "body": "Full coordinating artifact body.",
      "labels": ["code", "ready"],
      "state": "open",
      "workflow_kind": "code"
    },
    "lineage": [],
    "validation_scope": [],
    "optional_references": [],
    "truncation": {
      "depth_exceeded": false,
      "count_exceeded": false,
      "content_truncated": false
    }
  }
}
```

Omitting `artifact_context` is the explicit backward-compatible v1 form; the
`work_item` shape does not change when the bundle is added. `trace_context` is
also optional and contains only validated W3C `traceparent`/`tracestate` for this
assignment. The agent does not render it into the model prompt. Separate later
runs are linked by correlation/session identity rather than retaining this
context as a multi-day parent.

## Forge context tool channel

When the worker has a daemon context host, every role receives two read-only
tools:

- `forge_get_item {repo, number, type?, include_comments?}`;
- `forge_list_related {repo, number, type?, relations, depth?, limit?}`.

The child sends a `ForgeContextRequest` to the loopback address:

```json
{"protocol_version":1,"operation":{"operation":"forge_get_item","repo":"ai/temper","number":285,"type":"issue","include_comments":false}}
```

The request intentionally has **no** `worker_id`, `job_id`, credential, URL, or
generic method field. Unknown fields are rejected. The worker binds the current
job id and its own identity/authentication, forwards a worker-protocol
`FetchContext` through the configured carrier, validates the echoed response
identity, and returns only a bounded result or stable error:

```json
{"protocol_version":1,"status":"error","code":"not_authorized"}
```

Calls may be repeated to follow an indirect relation. The host enforces maximum
request size (1 MiB), a 30-second I/O/response timeout, configured repository
authorization, operation depth/count limits, item body/comment limits, and a
hard response bound. Stable errors are `invalid_request`, `not_authorized`,
`not_found`, `forge_unavailable`, and `limit_exceeded`. Backend diagnostics and
secrets are never included.

If no Forge context host was configured, the tools are absent rather than
present-but-broken. Initial `artifact_context` delivery is independent of tool
availability.

## Credentials and authority

The agent never receives Forge API or worker-pool credentials. Forge reads cross
the worker-owned channel; Forge mutations remain daemon-owned. Git push identity
is stored by the worker in the writable checkout's local Git configuration.

Exactly one protocol secret may enter the agent environment:
`TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON`, the model-provider credential. All
other process inputs are flags or files. In particular, the Forge loopback
address is not an authentication secret and is useful only while its bound
assignment is active.

## Terminal result

The agent writes one `WorkspaceResult` to `--result`. A writable head result may
carry `title`, `body`, and `summary`; verdict actions carry a declared `verdict`
and any contract-required authored content. The worker validates the result,
workspace diff, accepted `submit_for_pr` proof, and PR-head freshness before it
publishes the daemon `Result` message.

See also the [Worker/Daemon wire protocol](worker-daemon-wire-protocol.md) for
`FetchContext`/`ContextResponse` and
[Configure the coding agent](../how-to/configure-coding-workspace.md) for
deployment setup.
