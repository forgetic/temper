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
  `model_connect_timeout_secs`, and `model_idle_timeout_secs` values;
- `--agent-lifecycle-address <HOST:PORT>` for known first-party commands only.
  This is a dedicated correctness channel, not an activity-trace endpoint;
- `--terminal-output <FILE>` for known first-party commands only. The worker
  creates this private, per-run destination for a bounded typed model failure
  when the agent cannot write a `WorkspaceResult`.

The worker classifies each invocation as first-party or third-party. Known
`temper-agent` and `temper agent` commands receive complete resolved limits and
first-party lifecycle flags. Explicit third-party profile commands receive no
Temper-specific limits flag and remain under bounded worker fallback
supervision. Profile deadline overrides inherit field-by-field from
`[agent.deadlines]` before the file is written.

### Codebase-memory provider contract

When `--tool-config` enables `codebase_memory`, Temper supports
`codebase-memory-mcp` version 0.9.0 or newer. Initialization must identify that
provider in `serverInfo`, advertise the MCP `tools` capability, and expose:

- `index_status` with a required string `project` input and a bounded JSON
  result that explicitly classifies the requested identity as missing, stale,
  or fresh; and
- `index_repository` with required string `repo_path` and a string `name`
  input. Version 0.9.0 defines `name` as the stable, idempotent upsert identity,
  so concurrent calls with the same name converge rather than create
  path-keyed projects.

Temper retains only bounded initialize metadata (provider/protocol strings and
top-level capability names) and validates the advertised tool schemas before
project discovery. It derives the provider project key from a SHA-256 digest of
the host-controlled repository `id`, `owner`, and `name`; checkout path, branch,
role, correlation key, and work item are not inputs. Model-facing aliases stay
limited to prepared-workspace repository identities and are translated to that
provider project key internally.

Startup calls `index_status` only for those derived identities; it never reads
an unfiltered provider inventory. Only an explicit missing or stale result may
start indexing. A timeout, malformed response, unknown status, or incompatible
provider marks discovery unavailable and causes zero `index_repository` calls.
Auto mode either disables an incompatible provider with upgrade guidance or,
for an unknown discovery result, exposes a fresh read-only MCP client with the
skip reason in prompt status. Required mode fails setup. Neither mode falls
back to path-keyed creation.

The side-channel listeners exist only for that run, accept bounded JSON, and
are stopped when the child exits. Standalone calls the same host callbacks
in-process rather than opening sockets. Every carrier binds the callbacks to
the exact assignment attempt ID and its shared `AttemptFence`. Ownership loss
closes that fence before cancellation starts. From that point onward, the only
permitted work is joined cleanup, durable cancellation tracing/result recording,
and forwarding those terminal records; late model, tool, Forge, submission,
workspace, validation, commit, push, and ordinary-result completions are
discarded.

The worker writes per-run protocol files in a private temporary directory. Its
durable result root is created with owner-only permissions below
`[paths].state_dir` when available, or below the worker workspace root otherwise.

## Always-on lifecycle channel

A first-party child opens the attempt-owned loopback endpoint and sends a
newline-delimited `AgentLifecycleHelloV1` followed by monotonic
`AgentLifecycleFrameV1` values. The first sequence is `1`. A frame contains only
`version`, `seq`, opaque scope identity, and one closed event:

- `model_started`, `model_progress`, `model_finished`, `model_retrying`;
- `tool_started`, `tool_finished`;
- `steering_applied`, `agent_finished`;
- `containment`, carrying bounded managed-bash/MCP cleanup evidence.

For example:

```json
{"version":1,"seq":7,"scope":{"id":"scope-a","parent_id":"scope-root"},"event":{"type":"tool_started","call_id":"call-4","name":"grep"}}
```

Frames cannot carry prompt text, model output/thinking, tool arguments/results,
credentials, worker ID, or job ID. IDs and tool names are non-empty and bounded;
frames have a 64 KiB hard limit and reject unknown fields. The worker binds the
endpoint to `AgentRunRequest.attempt_id`, stamps receipt with its runtime clock,
ignores duplicate sequence numbers and stale attempts, and closes a connection
on malformed, oversized, or gapped input. Fakes and the standalone runner use
the same typed `JobProgressReporter` without opening a socket.

Cancellation does not depend on a child-authored terminal frame. The reverse
command is `AgentLifecycleCommandV1::Cancel { stage, reason }`; `stage` is the
monotonic `graceful`, `forced_termination`, or `hard_kill` vocabulary and
defaults to graceful for older version-one peers. A receiver consumes every
intermediate stage in order even if publication advances while it is not being
polled. Forced and hard stages dispatch immediately to the attempt-owned
emergency process registry, independently of the native agent future and any
blocked MCP request mutex, before normal callbacks/acknowledgement handling.

After the joined supervisor has completed graceful exit or strongest-backend
termination and descendant cleanup, the worker appends one synthetic canonical
`run.finished` activity with `status=cancelled` and
`stop_reason=cancelled`. The host-only record is durable even when the child
never connected, stopped responding, or was killed. Repeating terminal
insertion returns the original cancelled sequence, so out-of-process cleanup
can write the boundary early without losing the sequence needed by the outer
forwarding wait. The in-process runner uses the same ordering: it requests
native model/tool cancellation, waits for the native task group and managed
effects to join, persists `RunFinished(Cancelled)`, waits for trace forwarding
to acknowledge that exact sequence, and only then returns attempt quiescence.

Nested `containment` frames remain content-free and assignment-neutral. Their
owner includes kind, opaque tool/command ID, backend and bounded root plus the
additive optional `root_pid`; older senders that omit `root_pid` remain valid.
Blocked observations carry phase, repeated-failure count, bounded TERM/KILL
attempts and survivor process identities. The worker binds them to the exact
attempt and computes first-seen/age at its trusted boundary, so throttled
shutdown diagnostics can retain a root PID even when later discovery fails.

The ordinary success/failure path may give forwarding a bounded 250 ms flush
opportunity without changing the product result. That timeout is never used for
ownership-loss cancellation. While a durable cancellation sequence is
unacknowledged, the attempt is `cleanup_pending`: its closed fence, task-registry
entry, heartbeat membership, daemon slot, and worker permit remain occupied,
and no canceled result is recorded. Transport, daemon, storage-cursor, and
forwarding failures leave this state intact while the existing forwarder retry
and startup-recovery passes drain the spool. Capture `off` is the sole explicit
no-trace compatibility case. Thus neither carrier can expose a quiescent
cancelled run whose journal query still reports `active`.

The ordinary ownership-loss path has no fail-open deadline: it waits for both
recursive-empty/direct-child proof and terminal acknowledgement before
`AttemptQuiesced`, result recording, heartbeat removal, or permit release.
`temper serve standalone` process shutdown is deliberately different. Its one
absolute `deployment.standalone_shutdown_budget_secs` interval may end in
`bounded_crash_handoff`: durable assignments and trace spool are retained,
attempt-owned emergency KILL runs out of band, and the process exits immediately
with distinct status 70 without claiming local quiescence. The core-dump-free
primitive does not unwind, invoke C/Rust exit handlers or owner drops, or flush
userspace buffers. Replacement startup convergence handles the retained work,
while the old attempt fence continues to reject result and Forge effects.

`standalone.shutdown.blocker` correlates the protocol evidence as one of
`containment`, `terminal_trace_ack`, `result_delivery`, `component_task`, or
`registry_state`, with worker/job/attempt identity, owner root/name, root PID,
survivor PIDs, containment phase or trace run/sequence, first-seen age,
escalation stage, and deadline remaining. The terminal summary distinguishes
`graceful_exit` from `bounded_crash_handoff`; missing root/PID evidence is never
recursive-empty proof.

The producer maps model attempt boundaries, tool boundaries, steering, and
agent termination directly from `AgentEvent`. Non-empty text deltas and
completed streamed tool calls produce content-free `model_progress`, coalesced
to at most one frame per model call per five monotonic seconds. Empty text and
thinking-only deltas do not count. Main and nested agents use distinct opaque
scopes with explicit parent IDs.

Lifecycle production is installed beside activity normalization. It remains on
when trace capture is `off`, and it shares no trace queue, quota, spool, or
storage path; trace startup/storage failure therefore cannot disable progress.
The staged `AgentLifecycleCommandV1::Cancel` and
`AgentLifecycleCancellationAcknowledgementV1` values define the bounded reverse
cancellation handshake used by the process supervisor. Explicit third-party
commands receive no lifecycle flag and may emit nothing; worker fallback
supervision remains authoritative for them.

## Typed first-party terminal failure

A recognized first-party child that ends with a terminal model failure writes
an `AgentTerminalOutputV1` document to `--terminal-output`, even though no
`WorkspaceResult` exists:

```json
{"protocol_version":1,"model_failure":{"provider":"openai-codex","model":"gpt-safe","category":"rate_limit","retryable":true,"http_status":429,"provider_request_id":"req_748","provider_error_code":"rate_limit","message":"Provider rate limit reached.","detail_redacted":false}}
```

The document is closed, limited to 4 KiB, and contains only the normalized
`ModelFailureV1` vocabulary. It has no prompt, raw response, thinking, tool
content, credential, stderr, or generic extension field. The reference
`temper-agent` writes it only for `CodingAgentError::ModelFailure` and
`ModelUnavailable` after all in-run model retries are exhausted.

The worker provides and reads this private path only for commands already
recognized as first-party (the same commands that receive `--runtime-limits`).
It validates the version, complete JSON shape, field bounds, and safe diagnostic
before attaching the model failure to `AgentRunError`. It does this before the
ordinary nonzero-exit fallback. A third-party or legacy child never receives the
path. Missing, malformed, oversized, unsafe, and signal/crash outputs retain the
existing generic child-process failure. Stderr remains bounded operator text
only and is never parsed for category, retryability, provider/model identity,
status, request ID, or provider code.

## `WorkspaceContext` and artifact compatibility

`WorkspaceContext.artifact_context` is an optional versioned
`ArtifactContextBundle`. It contains the primary artifact, mandatory ancestry,
compact related indexes, directed relations, diagnostics, and explicit
truncation flags. Each full snapshot may also carry an optional `workflow`
projection containing only normalized parent/dependency references, branch and
correlation values, and persisted child identities. The agent renders the bundle
into stable sections (primary, lineage, validation summaries, optional
references, and diagnostics) instead of asking a model to interpret raw graph
JSON. The primary body is always rendered in full. When mandatory ancestor
bodies exceed 6,000 bytes in aggregate, the agent renders a deterministic
decision-dense projection: constraints, architecture, risks, decisions,
compatibility, evidence, and non-goals take precedence over repeated objective,
acceptance, work-breakdown, validation-strategy, and test-mapping prose. The
wire bundle remains unchanged and still carries the complete bounded snapshots.
Small lineage bundles remain byte-for-byte authored.

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
      "workflow_kind": "code",
      "workflow": {
        "kind": "code",
        "parents": [
          {"repository_id": "repo-1", "number": 277}
        ],
        "dependencies": [
          {"repository_id": "repo-2", "number": 88}
        ],
        "target_branch": "main",
        "correlation_key": "context-for-code-285",
        "children": [
          {
            "repository_id": "repo-1",
            "number": 286,
            "title": "Render compact workflow context",
            "state": "open"
          }
        ]
      }
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
`work_item` shape does not change when the bundle is added. Within a full
snapshot, `workflow` is also optional and `workflow_kind` remains available for
legacy producers and consumers. A workflow projection has no generic payload or
nested metadata field: leases, assignments, create intents, source bodies,
completion/staging/wiring state, and encoded body payloads cannot cross this
boundary. `trace_context` is also optional and contains only validated W3C
`traceparent`/`tracestate` for this assignment. The agent does not render it into
the model prompt. Separate later runs are linked by correlation/session identity
rather than retaining this context as a multi-day parent.

## Forge context tool channel

When the worker has a daemon context host, every role receives two read-only
tools:

- `forge_get_item {repo, number, type?, include_comments?}`;
- `forge_list_related {repo, number, type?, relations, depth?, limit?}`.

The child sends a `ForgeContextRequest` to the loopback address:

```json
{"protocol_version":1,"operation":{"operation":"forge_get_item","repo":"ai/temper","number":285,"type":"issue","include_comments":false}}
```

The request intentionally has **no** `worker_id`, `job_id`, assignment attempt
ID, credential, URL, or generic method field. Unknown fields are rejected. The
worker binds the current job ID, exact attempt ID, and its own
identity/authentication, populates `FetchContext.attempt_id`, and forwards the
request through the configured carrier. The host checks the shared attempt
fence both before transport starts and after it completes. A call that begins
while owned but completes after ownership loss returns stable
`forge_unavailable`; its late successful payload is never accepted. Only then
does the worker validate the echoed response identity and return a bounded
result or stable error:

```json
{"protocol_version":1,"status":"error","code":"not_authorized"}
```

Calls may be repeated to follow an indirect relation. With
`include_comments=false` (the default), `forge_get_item` omits comments. With
`include_comments=true`, it exposes a bounded projection of ordinary issue or
pull-request conversation comments, including durable plan-validation audits;
comment content and counts remain subject to the host limits below. It does not
expose Forgejo label changes, provider activity/timeline records, or hidden
comment rows because the portable Forge abstraction has no such read operation.
To investigate a plan-validation result, agents should request the coordinating
plan with comments and locate its stable
`temper:comment-key=plan-validation:<assignment-key>` audit. The assignment key
is a SHA-256 digest of the exact job/attempt identity; use the separately
rendered Job ID and Attempt ID fields when selecting among repeated validation
rounds. Do not infer history from Temper journals, Forgejo SQLite, or timeline
internals.

The host enforces maximum request size (1 MiB), a 30-second I/O/response timeout,
configured repository authorization, operation depth/count limits, item
body/comment limits, and a hard response bound. Stable errors are
`invalid_request`, `not_authorized`, `not_found`, `forge_unavailable`, and
`limit_exceeded`. Backend diagnostics and secrets are never included.

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
and any contract-required authored content. A first-party model failure that has
no workspace result uses the separate private typed terminal carrier above; it
does not synthesize a `WorkspaceResult`. The worker validates the result,
workspace diff, accepted `submit_for_pr` proof, and PR-head freshness before it
publishes the daemon `Result` message. In-process `submit_for_pr` uses the same
attempt fence and cancellation handle as the model run: the gate and controlled
fingerprint are checked at entry and completion, managed commands are joined on
cancellation, and a fenced call clears accepted proof and returns
`accepted=false` with `agent attempt is no longer available`. Commit, push,
validation, result conversion, and publication recheck the same fence so a late
successful future cannot restore authority.

See also the [Worker/Daemon wire protocol](worker-daemon-wire-protocol.md) for
`FetchContext`/`ContextResponse` and
[Configure the coding agent](../how-to/configure-coding-workspace.md) for
deployment setup.
