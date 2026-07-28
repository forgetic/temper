# Worker/Daemon Wire Protocol v1

The Temper worker/daemon wire protocol defines the language-neutral boundary
between a Temper daemon and external Smith-style workers. Workers long-poll the
daemon for assigned work, run LLM or coding jobs, push git branches as their
configured role identity, and return structured results. The daemon remains the
only component that talks to the Forge API: it owns webhooks, scans and polls,
leases, API mutations, per-role credentials, PR create/update, and worker
registry state.

The protocol shape is frozen by
`docs/reference/worker-daemon-wire-protocol/schema.json` and the canonical JSON
fixtures in `docs/reference/worker-daemon-wire-protocol/examples/`.

## Transport and invocation

The north star is to generate both sides from a language-neutral spec, allowing
a future forgejo-runner-style gRPC/protobuf transport without changing message
semantics. The interim implementation direction may introduce one tiny, stable,
independently versioned protocol crate carrying versioned JSON DTOs as the only
allowed Smith-to-Temper dependency.

Version 1 wire framing is HTTP long-poll with JSON request and response bodies.
Workers call daemon endpoints to register, poll for work, send heartbeats,
return results, and acknowledge lease release. The daemon replies to `poll` with
one `assign` message when work is available, or an `error` response such as
`poll_timeout` when the long-poll window expires. This preserves the pull model:
workers initiate all worker/daemon exchanges, and the daemon never opens
connections to workers or pushes unsolicited jobs to them.

JSON keeps v1 easy to inspect and fixture-test while preserving message
semantics for a future protobuf/gRPC transport. The additive verdict-job fields
`JobContext.action`, `JobContext.checkout_capability`,
`JobContext.allowed_verdicts`, `JobContext.verdict_contracts`,
`JobContext.source_metadata`, `JobContext.artifact_context`, legacy
`JobContext.guidance`, additive `JobContext.structured_guidance`, `JobResult.verdict`, `JobResult.body`,
`JobResult.children`, `JobResult.children[].kind`,
`JobResult.failure.model_failure`, and `JobResult.failure.session_recovery` are all optional, and the
protocol version remains `1`. The `fetch-context`/`context-response`,
`activity-batch`/`activity-ack`, and daemon-to-worker `cancel-attempts`
capabilities are additive in the same v1 envelope.

## Envelope

Every message is one JSON object with these top-level envelope fields:

| Field | Type | Required | Semantics |
| --- | --- | --- | --- |
| `protocol_version` | integer | yes | Must be `1` for this version. A different value is a breaking protocol mismatch. |
| `type` | string | yes | Discriminator identifying the message variant. |

Each message carries its required payload fields at the top level. The schema
models the messages as a JSON Schema draft 2020-12 discriminated union on
`type`.

## Messages

### `register` — worker → daemon

Worker announces its identity, routing capabilities, optional reserved labels,
and capacity.

| Field | Type | Required | Semantics |
| --- | --- | --- | --- |
| `protocol_version` | integer | yes | Constant `1`. |
| `type` | string | yes | Constant `register`. |
| `worker_id` | string | yes | Stable identity for this worker process or installation. |
| `capabilities` | array | yes | Non-empty list of role/repository routes this worker can run. |
| `capabilities[].role` | string | yes | Workflow role id, for example `coder` or `architect`. |
| `capabilities[].repo` | string | yes | Repository slug, for example `ai/temper`. |
| `capacity` | object | yes | Worker capacity declaration. |
| `capacity.max_concurrent_jobs` | integer | yes | Maximum concurrent jobs; must be at least `1`. |
| `labels` | array of strings | no | Reserved for later label-indirection routing. v1 readers must tolerate it and may ignore it. |

### `poll` — worker → daemon

Worker requests work and reports current free capacity. The daemon holds the
request open until work is available or the poll timeout expires.

| Field | Type | Required | Semantics |
| --- | --- | --- | --- |
| `protocol_version` | integer | yes | Constant `1`. |
| `type` | string | yes | Constant `poll`. |
| `worker_id` | string | yes | Registered worker id. |
| `free_capacity` | integer | yes | Current free capacity; must be at least `0`. |
| `max_wait_ms` | integer | no | Requested long-poll timeout in milliseconds. The daemon may cap it according to deployment policy. |

If work is available before timeout, the daemon returns one `assign` message. If
no work is available before timeout, the daemon returns an `error` message with
code `poll_timeout`; the worker should immediately re-poll unless shutting down.

### `assign` — daemon → worker

Daemon assigns one concrete role/action job to a worker. The assignment is not a
request for the worker to choose among workflow actions; the selected action,
when known, is carried in the payload.

| Field | Type | Required | Semantics |
| --- | --- | --- | --- |
| `protocol_version` | integer | yes | Constant `1`. |
| `type` | string | yes | Constant `assign`. |
| `trace_context` | object | no | Validated W3C context for this assignment delivery. It is not persisted as a multi-run workstream parent. |
| `trace_context.traceparent` | string | yes when `trace_context` is present | Canonical lowercase W3C `traceparent`. |
| `trace_context.tracestate` | string | no | Bounded W3C `tracestate` (at most 512 bytes). |
| `job_id` | string | yes | Daemon-generated unique job id. |
| `attempt_id` | string | yes | Opaque daemon-generated identity for this dispatch attempt. Workers must copy it unchanged into results and heartbeats. Compatibility readers may deserialize legacy assignments without it, but current workers refuse them. |
| `role` | string | yes | Workflow role id for the assignment. |
| `repo` | string | yes | Repository slug. |
| `artifact` | object | yes | Target work item identity. |
| `artifact.item` | string or number | yes | Artifact identity matching the daemon's work-item context representation for this forge. |
| `artifact.kind` | string | yes | Artifact kind, for example `intake`, `issue`, or `pull_request`. |
| `job_payload` | object | yes | Arbitrary JSON object containing the assigned role/action job context required by the worker. |

The daemon, not the worker, holds the Forge lease/CAS token for the assignment,
following ADR 0013. If duplicate dispatch occurs, the lease/CAS remains the
arbiter. A losing completion or stale mutation harmlessly no-ops rather than
corrupting state. Workers must not call the Forge API to mutate artifacts or PRs.

### Standard job payload

`Assign.job_payload` remains opaque at the envelope and schema layer: the daemon
routes it as arbitrary JSON and workers must not depend on the envelope enforcing
a particular object shape. Temper daemons use a standard v1 object for scanned
workflow jobs so Smith-style workers can run without Forge API access:

| Field | Type | Required | Semantics |
| --- | --- | --- | --- |
| `trace_context` | object | no | Same optional W3C assignment context as the envelope; the worker rejects conflicting copies and propagates the value into `WorkspaceContext`. |
| `role` | string | yes | Workflow role id. |
| `repo` | string | yes | `owner/name` repository slug used for worker routing. |
| `queue` | string | yes | Workflow queue id. |
| `artifact_kind` | string | yes | Workflow artifact kind id. |
| `repository` | object | no | Repository coordinates for the job assignment. |
| `repository.owner` | string | yes when `repository` is present | Repository owner. |
| `repository.name` | string | yes when `repository` is present | Repository name. |
| `repository.default_branch` | string | yes when `repository` is present | Forge repository default branch as reported by the backend. |
| `base_branch` | string | no | Workspace base branch for checkout and implementation PR target. Defaults to the normalized Forge default branch, but workflow metadata `target_branch` may override it for issue-backed implementation work. For writable issue-backed checkouts, if this differs from the repository default branch and is absent on the Forge, the worker creates it from the default branch before checking out the work branch; read-only siblings and PR-head modes do not materialize branches. |
| `branch_hint` | string | no | Deterministic worker branch suggestion, for example `agent/pr-for-code-42`. |
| `correlation_key` | string | no | Deterministic PR correlation key, for example `pr-for-code-42`. |
| `artifact` | object | no | Enqueue-time issue snapshot. Omitted for older minimal payloads and for PR-targeted jobs in v1. |
| `artifact_context` | object | no | Versioned bounded graph bundle with explicitly separated primary, mandatory ancestry, validation scope, optional references, diagnostics, and truncation flags. Additive; legacy workers may ignore it. |
| `artifact_context.version` | integer | yes when `artifact_context` is present | Artifact-context schema version, currently `1` and independent of the worker protocol version. |
| `artifact_context.repository` | object | yes when `artifact_context` is present | Stable id and configured `owner/name` path of the coordinating repository. |
| `artifact_context.artifact_type` | string | yes when `artifact_context` is present | `issue` or `pull_request`. |
| `artifact_context.primary` | object | yes when `artifact_context` is present | Full coordinating snapshot; primary identity never depends on a vector position. Its optional `workflow` projection is compact and protocol-owned; `workflow_kind` remains for compatibility. |
| `artifact_context.primary.workflow.kind` | string | no | Normalized workflow artifact kind. |
| `artifact_context.primary.workflow.parents` | array | no | Fully qualified `{repository_id, number}` parent references. |
| `artifact_context.primary.workflow.dependencies` | array | no | Fully qualified `{repository_id, number}` dependency references. |
| `artifact_context.primary.workflow.target_branch` | string | no | Workflow-selected implementation target branch. |
| `artifact_context.primary.workflow.correlation_key` | string | no | Workflow correlation key. |
| `artifact_context.primary.workflow.children` | array | no | Persisted child identities containing only stable repository id, artifact number, title, and optional state. |
| `artifact_context.lineage` | array | no | Full mandatory ancestor snapshots in deterministic root-to-leaf order, excluding the primary. |
| `artifact_context.validation_scope` | array | no | Body-omitted declared dependencies and verified implementation PRs. Each summary retains labels, state, optional workflow kind, relation type, and the source artifact that exposed it. |
| `artifact_context.optional_references` | array | no | Body-omitted Markdown references, with the same self-describing summary metadata. |
| `artifact_context.diagnostics` | array | no | Non-fatal partial-collection diagnostics. Dispatch continues when only related context is unavailable. |
| `artifact_context.truncation` | object | yes when `artifact_context` is present | Explicit `depth_exceeded`, `count_exceeded`, and `content_truncated` booleans. |
| `artifact.number` | integer | yes when `artifact` is present | Repository-scoped issue number. |
| `artifact.title` | string | yes when `artifact` is present | Issue title at enqueue time. |
| `artifact.body` | string | yes when `artifact` is present | Issue body at enqueue time. |
| `artifact.labels` | array of strings | yes when `artifact` is present | Issue labels at enqueue time. |
| `artifact.state` | string | yes when `artifact` is present | Debug-formatted issue state, for example `Open`. |
| `action` | string | no | Workflow action (intent-level tool / transition id) this job services, for example `open_pr` or `triage_intake`. |
| `checkout_capability` | string | no | Checkout capability the worker should prepare: `writable`, `read_only`, `pull_request_read_only`, or `pull_request_writable`. Absent means writable, preserving v1's original behavior. |
| `allowed_verdicts` | array of strings | no | Verdict vocabulary declared by `action`'s `outcomes` keys, in deterministic order. Empty or absent for a plain coding job. |
| `verdict_contracts` | object | no | Workflow-derived result requirements keyed by verdict: child cardinality/kinds, required child/source metadata, resolved child `target_branch`, and required PR text or authored body. A resolved branch names the exact accepted value, the repository default used for comparison, and whether omission authorizes engine stamping. Older contracts omit this additive requirement. Required child metadata must appear non-blank in each child body's `<!-- temper:workflow ... -->` JSON block. |
| `source_metadata` | object | no | Parsed assignment-time source metadata used by worker/agent preflight validation. The engine re-reads current Forge state before mutation. |
| `guidance` | string | no | Legacy v1 free-text prompt guidance retained for existing workers. New daemons flatten every configured guidance category into this string, with category headings where needed. |
| `structured_guidance` | object | no | Additive lossless prompt guidance for updated workers. `role_guidance` composes the selected role charter and role prompt guidance; `tool_guidance` and `tool_constraints` preserve the applicable declared external tool's instructions; `action_guidance` carries queue-action prose followed by any generated PR-repair details. Updated workers prefer this field when both carriers are present. |

For compatibility, old minimal payloads containing only `role`, `repo`, `queue`,
and `artifact_kind` remain valid; the enrichment fields are optional. The
`action`, `checkout_capability`, `allowed_verdicts`, `verdict_contracts`,
`source_metadata`, `artifact_context`, `guidance`, and `structured_guidance`
additions are also optional, and adding them does not change the protocol version: it
remains `1`. New daemons keep `guidance` in its original string shape and put the
categorized representation in `structured_guidance`, so v1 readers may ignore the
new field without encountering a type change.
The legacy singular `artifact` object is retained unchanged when
`artifact_context` is present; consumers must not require one carrier to replace
the other. Readers must ignore unknown fields in the standard payload just as they do for
protocol messages.

### `fetch-context` — worker → daemon

An active worker requests one bounded, read-only Forge lookup for its current
assignment. It sends the message through the same carrier as poll/result traffic:
`POST /v1/message` with worker-pool bearer authentication in split deployments,
or the co-resident in-process transport in standalone mode. Both carriers reach
the same authorization and retrieval implementation.

| Field | Type | Required | Semantics |
| --- | --- | --- | --- |
| `protocol_version` | integer | yes | Constant `1`. |
| `type` | string | yes | Constant `fetch-context`. |
| `worker_id` | string | yes | Registered worker identity, at most 256 bytes. |
| `job_id` | string | yes | An assignment currently active on that worker, at most 256 bytes. |
| `attempt_id` | string | yes | Exact attempt fence copied from the assignment, at most 256 bytes. Omission is accepted only when deserializing a legacy request. |
| `operation` | object | yes | Exactly one closed-vocabulary operation described below. |

`forge_get_item` accepts `repo`, positive `number`, optional `type`
(`issue`/`pull_request`), and `include_comments` (default `false`). Setting it to
`true` returns bounded ordinary issue/PR conversation comments. This includes a
coordinating plan's durable validation-audit comment, but excludes Forgejo label
changes and provider timeline/activity records: the portable Forge abstraction
does not expose those records. Operators and agents should use the stable
`temper:comment-key=plan-validation:<assignment-key>` ordinary comment. Its key
is derived from the exact job and attempt identity, so repeated rounds with one
job ID remain distinct and exact replay remains idempotent. The visible audit
renders both IDs. Do not use Temper journals, Forgejo SQLite, or hidden timeline
rows when reconstructing a validation outcome.

`forge_list_related` accepts `repo`, positive `number`, optional `type`, a
non-empty unique subset of `parent`, `child`, `dependency`, `dependent`,
`produced_pr`, `body_reference`, and `referenced_by`, plus optional bounded
`depth` and `limit`. Repeated calls are supported so a client can deliberately
follow indirect relations without one unbounded graph request.

The daemon authorizes the worker-pool credential, exact
`(worker_id, job_id, attempt_id)` active-assignment binding, and configured
repository before any Forge read. A legacy omitted attempt id compares as exact
`None`, never as a wildcard for a current fenced attempt. Pending, completed,
another worker's, another attempt's, and unconfigured-repository reads are
`not_authorized`. The operation is read-only; mutation names are invalid.

### `context-response` — daemon → worker

Every accepted `fetch-context` envelope receives exactly one tagged outcome with
the request identity echoed back. Workers must reject a response whose version,
worker id, or job id differs from the request.

| Field | Type | Required | Semantics |
| --- | --- | --- | --- |
| `protocol_version` | integer | yes | Constant `1`. |
| `type` | string | yes | Constant `context-response`. |
| `worker_id` | string | yes | Echo of the request worker. |
| `job_id` | string | yes | Echo of the request assignment. |
| `status` | string | yes | Exactly `success` or `error`. |
| `result` | object | success only | Tagged `item` or `related` result with bounded content and truncation flags. |
| `code` | string | error only | One stable public code: `invalid_request`, `not_authorized`, `not_found`, `forge_unavailable`, or `limit_exceeded`. |

The daemon never serializes backend error details or credentials. Responses are
hard-bounded; item bodies/comments and relation counts are truncated
predictably, with truncation dimensions reported in the result. See
[`fetch-context.json`](worker-daemon-wire-protocol/examples/fetch-context.json),
[`context-response.json`](worker-daemon-wire-protocol/examples/context-response.json),
and the stable error example
[`context-response-error.json`](worker-daemon-wire-protocol/examples/context-response-error.json).

### `activity-batch` — worker → daemon

The worker forwards one contiguous, durably spooled canonical activity batch.
The shared activity DTO remains owned by `temper-protocol-activity`; this
worker-protocol envelope adds authenticated transport identity and the immutable
assignment binding used by the engine journal.

| Field | Type | Required | Semantics |
| --- | --- | --- | --- |
| `protocol_version` | integer | yes | Constant `1`. |
| `type` | string | yes | Constant `activity-batch`. |
| `worker_id` | string | yes | Registered worker that stamped and spooled the canonical events. |
| `assignment_id` | string | yes | Durable assignment/attempt identity; currently the worker job id. |
| `capture_policy` | object | yes | Versioned capture/quota policy used when the spool was written. |
| `batch` | object | yes | Shared `AgentActivityBatch`: one run id, contiguous events, and exactly the referenced blob attachments. |

Split HTTP delivery uses `POST /v1/message` and is disabled unless worker-pool
authentication is configured. The daemon verifies the bearer credential,
registration, worker capability for the event role/repository, envelope worker
identity, and immutable event identity before journal ingestion. The co-resident
carrier is trusted but still requires a registered, capable worker. Forwarding
continues independently of job-result delivery, so a restarted worker can drain
old terminal spools without rerunning an agent.

### `activity-ack` — daemon → worker

The daemon returns this response only after the journal has appended and synced
all newly accepted contiguous records. Lost replies are safe: the worker resends
the same batch and the journal deduplicates by `(run_id, seq)`.

| Field | Type | Required | Semantics |
| --- | --- | --- | --- |
| `protocol_version` | integer | yes | Constant `1`. |
| `type` | string | yes | Constant `activity-ack`. |
| `worker_id` | string | yes | Echo of the authenticated worker identity. |
| `acknowledgement` | object | yes | Shared acknowledgement containing `version`, `run_id`, and `highest_contiguous_seq`. |

The worker rejects mismatched or out-of-range acknowledgements and advances its
atomic spool cursor only through `highest_contiguous_seq`. Count and encoded-byte
batch limits, capped retry backoff, and a bounded terminal flush keep trace
outages non-fatal to product work. See
[`activity-batch.json`](worker-daemon-wire-protocol/examples/activity-batch.json)
and [`activity-ack.json`](worker-daemon-wire-protocol/examples/activity-ack.json).

### `heartbeat` — worker → daemon

Worker reports liveness and progress for in-flight jobs so the daemon can detect
stalls and reclaim leases.

| Field | Type | Required | Semantics |
| --- | --- | --- | --- |
| `protocol_version` | integer | yes | Constant `1`. |
| `type` | string | yes | Constant `heartbeat`. |
| `worker_id` | string | yes | Worker id. |
| `jobs` | array | yes | Per-job heartbeat objects. |
| `jobs[].job_id` | string | yes | Assigned job id. |
| `jobs[].attempt_id` | string | yes | Exact attempt fence from the assignment. Omission is tolerated only for legacy recovered metadata. |
| `jobs[].state` | string | yes | Job state: `running`, `waiting`, or `finishing`. |
| `jobs[].message` | string | yes | Short human-readable phase text; consumers should prefer `liveness` when present. |
| `jobs[].liveness` | object | no | Additive worker-owned structured report. Omitted by legacy/third-party workers. |
| `jobs[].liveness.phase` | string | yes when `liveness` is present | `running`, `cancel_requested`, `quiesced`, or `result_recorded`. |
| `jobs[].liveness.run_elapsed_ms` | integer | yes when `liveness` is present | Monotonic elapsed time since this attempt acquired its local permit. |
| `jobs[].liveness.no_progress_elapsed_ms` | integer | yes when `liveness` is present | Monotonic elapsed time since the last accepted agent lifecycle boundary. Lease heartbeats do not reset it. |
| `jobs[].liveness.active_operation_count` | integer | yes when `liveness` is present | Full number of parallel model/tool operations. |
| `jobs[].liveness.active_operations` | array | no | At most eight content-free summaries (`scope`, `kind`, `name`, `operation_id`, `elapsed_ms`). The count may exceed the array length. |
| `jobs[].liveness.timeout` | object | no | Winning timeout `reason` (`no_progress` or `max_run`) and configured `limit_ms`. |
| `jobs[].liveness.cancellation` | string | yes when `liveness` is present | `not_requested`, `requested`, `escalated`, or `quiesced`. |
| `jobs[].liveness.result_durability` | string | yes when `liveness` is present | `none`, `pending`, or `durable`. |
| `jobs[].liveness.result_delivery` | string | yes when `liveness` is present | `not_ready` or `pending`; delivery continues independently after permit release. |
| `jobs[].liveness.pending_result` | boolean | yes when `liveness` is present | Whether a terminal result is waiting for durable recording or delivery. |
| `free_capacity` | integer | no | Current free capacity; must be at least `0` when present. |

Heartbeat interval and missed-heartbeat threshold are deployment-configured
daemon policy, not fixed wire constants. If a worker misses the threshold, the
daemon may mark the worker unhealthy, reclaim leases, and reassign eligible work.
The structured report is observability only: the worker watchdog remains the
no-progress authority and durable Forge assignment metadata remains claim
authority. The daemon registry retains only the latest accepted report for each
exact `(worker_id, job_id, attempt_id)` and never renews a lease from its elapsed
values.

The operator `GET /v1/state` and `GET /v1/state/job/{job_id}` projections expose
the report additively. Registered workers have a `jobs` array of latest reports;
in-flight jobs include `attempt_id` and optional `worker_report`. An absent
report means unknown/legacy, not healthy or idle. Tool arguments, result bodies,
prompts, credentials, and model content cannot appear in these DTOs.

A normal successful heartbeat has no message body (`204` over HTTP / `None` in
the protocol API). This remains the common response and is unchanged by the
additive cancellation capability. When durable ownership of one or more exact
attempts is lost, the daemon may instead return `cancel-attempts`.

### `cancel-attempts` — daemon → worker

The response requests idempotent cancellation of one or more attempts reported
by the same worker heartbeat. Entries are unique exact identities and serialize
in deterministic `(job_id, attempt_id)` order.

| Field | Type | Required | Semantics |
| --- | --- | --- | --- |
| `protocol_version` | integer | yes | Constant `1`. |
| `type` | string | yes | Constant `cancel-attempts`. |
| `worker_id` | string | yes | Worker whose heartbeat is being answered. |
| `cancellations` | array | yes | Non-empty list of unique exact cancellation entries. |
| `cancellations[].worker_id` | string | yes | Must equal the envelope `worker_id`. |
| `cancellations[].job_id` | string | yes | Exact assigned job identity. |
| `cancellations[].attempt_id` | string | yes | Exact attempt fence. Omission is accepted only when reading legacy metadata and compares as `None`, never as a wildcard. Modern directives require a non-blank value. |
| `cancellations[].cause` | string | yes | Stable value `ownership_lost`. |
| `cancellations[].reason` | string | yes | Stable non-blank operator-facing reason, at most 512 UTF-8 bytes. |

Workers must match all three identity components exactly before acting. A stale
directive for another attempt is a no-op. Cancellation is idempotent: the daemon
may repeat the same directive on later heartbeats until that exact attempt
reports a terminal result. See the canonical multi-attempt fixture
[`cancel-attempts.json`](worker-daemon-wire-protocol/examples/cancel-attempts.json).

### `result` — worker → daemon

Worker returns the structured result for one assigned job.

| Field | Type | Required | Semantics |
| --- | --- | --- | --- |
| `protocol_version` | integer | yes | Constant `1`. |
| `type` | string | yes | Constant `result`. |
| `worker_id` | string | yes | Worker id. |
| `job_id` | string | yes | Assigned job id. |
| `attempt_id` | string | yes | Exact attempt fence from the assignment. An unfenced result cannot complete a current fenced assignment. |
| `status` | string | yes | `success` or `failure`. |
| `branch` | object | required for successful code-producing jobs | Pushed branch data. |
| `branch.name` | string | yes when `branch` is present | Pushed branch name. |
| `branch.head_sha` | string | yes when `branch` is present | Git commit SHA at the branch head. |
| `verdict` | string | no | Verdict chosen by a verdict job. Must be one of the assignment payload's `allowed_verdicts` when present. |
| `title` | string | no | Agent-authored PR title. Without a verdict this is the implementation PR handoff title. With a verdict it is used only when the routed transition declares a metadata-driven `create_pull_request` PR artifact kind. |
| `body` | string | no | Authored body accompanying a verdict, such as a rewritten issue spec or PR review body. |
| `children` | array of objects | no | Workspace-authored child issues for breakdown verdicts such as `needs_breakdown`. Empty or absent means no children. |
| `children[].slug` | string | yes | Stable per-child identifier within the result; seeds the child's correlation key and is referenced by sibling `depends_on` entries. |
| `children[].title` | string | yes | Child issue title. |
| `children[].body` | string | yes | Child issue body. |
| `children[].kind` | string | no | Workflow artifact kind to create, for example `code`, `plan`, or `validation`. Omitted defaults to `code` for backward compatibility. The daemon derives required identifying labels, and non-conflicting initial labels, from this kind. |
| `children[].labels` | array of strings | no | Labels to apply when creating the child issue. Child-authored lifecycle/state labels are preserved; for example, a `code` child with `blocked` is not also given the default `ready` label. |
| `children[].depends_on` | array of strings | no | Slugs of sibling children in the same result that must land before this one. |
| `children[].target_repo` | string | no | Target repository as an `owner/name` path. Omitted means the assignment's own repository. |
| `failure` | object | required for failures | Failure details. |
| `failure.class` | string | yes when `failure` is present | `transient`, `permanent`, `canceled`, or `protocol`. |
| `failure.message` | string | yes when `failure` is present | Human-readable failure summary. |
| `failure.model_failure` | object | no | Normalized bounded `ModelFailureV1`, authoritative independently of activity tracing. Contains only safe provider/model identity, canonical disposition/provenance, optional status/request/code, a bounded sanitized message, and `detail_redacted`. |
| `failure.model_failure.disposition` | string | no for legacy diagnostics; yes for current workers | Canonical `retryable`, `non_retryable`, or `unknown` recovery authority. The compatibility `retryable` boolean is not an independent authority. |
| `failure.model_failure.boundary` | string | no for legacy diagnostics; yes for current workers | Typed `http`, `sse`, or `local` failure boundary. |
| `failure.model_failure.event_kind` | string | no for legacy diagnostics; yes for current workers | Closed safe event kind such as `http_response`, `stream_error`, or `stream_eof`. |
| `failure.model_failure.status_present` | boolean | no for legacy diagnostics; yes for current workers | Whether independently typed HTTP status evidence was present, even when no safe value is retained. |
| `failure.model_failure.code_present` | boolean | no for legacy diagnostics; yes for current workers | Whether provider code evidence was present, even when an unrecognized value is not retained. |
| `failure.session_recovery` | object | no | Typed bounded evidence for the durable session-recovery decision. |
| `failure.session_recovery.attempt_id` | string | yes when session recovery is present | Exact attempt identity; must match the enclosing result. |
| `failure.session_recovery.failure_epoch` | integer | yes when session recovery is present | One-based consecutive-failure epoch. |
| `failure.session_recovery.failure_count` | integer | yes when session recovery is present | One-based cumulative terminal count for the unsucceeded workstream epoch; session rotation never resets it. |
| `failure.session_recovery.session_number` | integer | no for legacy evidence; yes for current workers | One-based session number in the failure epoch. |
| `failure.session_recovery.session_failure_count` | integer | no for legacy evidence; yes for current workers | Terminal count in the session that produced this failure. |
| `failure.session_recovery.epoch_started_unix_ms` | integer | no for legacy evidence; yes for current workers | Failure-epoch wall-clock start. |
| `failure.session_recovery.epoch_elapsed_ms` | integer | no for legacy evidence; yes for current workers | Elapsed wall-clock evidence at the decision. |
| `failure.session_recovery.disposition` | string | no for legacy evidence; yes for current workers | Canonical `retryable`, `non_retryable`, or `unknown` recovery authority. |
| `failure.session_recovery.immediate_retry_exhausted` | boolean | no for legacy evidence; yes for current workers | Whether bounded same-turn model requests were exhausted. |
| `failure.session_recovery.configured_session_failure_limit` | integer | no for legacy evidence; yes for current workers | Snapshotted terminal-run budget for one session. |
| `failure.session_recovery.configured_fresh_session_limit` | integer | no for legacy evidence; yes for current workers | Snapshotted fresh-session budget. |
| `failure.session_recovery.configured_deferral_limit` | integer | no for legacy evidence; yes for current workers | Snapshotted provider-deferral budget. |
| `failure.session_recovery.deferral_count` | integer | no | Provider deferrals issued in this failure epoch. |
| `failure.session_recovery.deferral_generation` | integer | no | Monotonic deferred-wake fence generation. |
| `failure.session_recovery.not_before_unix_ms` | integer | required for `provider_deferred` | Earliest automatic provider-recovery wake. |
| `failure.session_recovery.slo_deadline_unix_ms` | integer | no for legacy evidence; yes for current workers | Absolute configured failure-epoch SLO boundary. |
| `failure.session_recovery.action` | string | yes when session recovery is present | `retry_current_session`, `rotate_session`, `provider_deferred`, or `park_for_human`. Provider deferral is automatic recovery and is distinct from actionable human parking. |
| `failure.session_recovery.current_session_id` | string | yes when session recovery is present | Session that produced the terminal failure. |
| `failure.session_recovery.prior_session_id` | string | no | Archived predecessor when the current session followed a rotation. |
| `failure.session_recovery.new_session_id` | string | no | Fresh session selected by `rotate_session`. |
| `failure.session_recovery.evidence_location` | string | yes when session recovery is present | Bounded worker-generated operator-safe path/location for durable evidence. |
| `summary` | string | no | Short result summary suitable for logs or operator display. |
| `details` | object | no | Arbitrary structured role-specific result details. |

The model diagnostic and session-recovery object are additive optional v1
fields. Workers normalize the model diagnostic at each agent/worker boundary;
daemons normalize it again at worker admission and discard invalid or
attempt-mismatched session evidence. These typed fields are never reconstructed
from failure messages, stderr, provider prose, prompts, or raw responses. Older
failure JSON without either field continues to deserialize unchanged. See
[`result-model-failure.json`](worker-daemon-wire-protocol/examples/result-model-failure.json).

The worker first records the exact result in its private durable result outbox.
Transport failures retain that entry and replay it with bounded exponential
backoff independently of job permits. The daemon applies a matching result
idempotently and does not acknowledge it until result bookkeeping and exact
durable-claim release complete. A duplicate exact delivery returns the prior
acknowledgement without reapplying. Permanent authentication/protocol rejection
moves the worker entry to operator-visible rejected storage.

The daemon performs any idempotent PR create/update through the Forge API as the
role identity. It also routes declared verdicts through the compiled workflow and
applies authored body, review, or child-issue effects when declared. The worker
never calls the Forge API for PR create/update or artifact mutation.

Verdict jobs are successful jobs whose result may carry `verdict` plus optional
`title`/`body` or breakdown `children` and no `branch`; the allowed vocabulary
comes from the assignment payload's `allowed_verdicts`. The daemon binds
`children` only when the routed verdict transition declares a `create_issues`
effect; a child's
optional `target_repo` uses the same `owner/name` shape as daemon `--repo` and
omits to the assignment's repository. Child `kind` defaults to `code`; when set,
it must name a workflow issue artifact kind that has at least one serviceable
queue in the active workflow. A queue is serviceable when it has automation, an
applicable role action, or a subscribed legacy role. The daemon rejects the
whole verdict before mutation when a child kind has no reachable queue/action.
The daemon stamps that kind into the child workflow metadata block when the body
lacks one, preserving existing metadata fields. An explicit non-empty child metadata `target_branch` is kept;
otherwise, a non-empty source issue `target_branch` is inherited into the child
metadata. If the body already carries a metadata
`kind`, it is preserved. For routed issue verdicts whose transition declares
`create_pull_request` with a PR `artifact_kind`, the daemon uses the source
issue's `target_branch` workflow metadata as the PR head branch, the repository
default branch as the PR base, derives labels from the named PR kind, and uses
`title`/`body` as the PR handoff when that is unambiguous. These result fields
are optional for backward compatibility, and their addition does not change the
protocol version: it remains `1`.

### `release` — daemon → worker

Daemon acknowledges that it has processed job completion and is releasing or
closing the assignment from the worker's perspective.

| Field | Type | Required | Semantics |
| --- | --- | --- | --- |
| `protocol_version` | integer | yes | Constant `1`. |
| `type` | string | yes | Constant `release`. |
| `worker_id` | string | yes | Worker id. |
| `job_id` | string | yes | Assigned job id. |
| `attempt_id` | string | yes | Attempt fence being acknowledged. The worker compacts an outbox entry only when this identity matches exactly. |
| `disposition` | string | yes | `accepted`, `superseded`, or `reclaimed`. `accepted` means the exact result applied and the claim converged; stale `superseded`/`reclaimed` acknowledgements compact without mutation and remain operator-visible warnings. |
| `message` | string | no | Human-readable explanation. |

### `lease-ack` — worker → daemon

Worker acknowledges the daemon's `release` and confirms local cleanup/lease
hand-back.

| Field | Type | Required | Semantics |
| --- | --- | --- | --- |
| `protocol_version` | integer | yes | Constant `1`. |
| `type` | string | yes | Constant `lease-ack`. |
| `worker_id` | string | yes | Worker id. |
| `job_id` | string | yes | Assigned job id. |
| `disposition` | string | yes | `released` or `unknown_job`. |
| `message` | string | no | Human-readable explanation. |

### `error` — either direction as a response envelope

Generic protocol-level error response.

| Field | Type | Required | Semantics |
| --- | --- | --- | --- |
| `protocol_version` | integer | yes | Constant `1`. |
| `type` | string | yes | Constant `error`. |
| `code` | string | yes | Stable machine-readable error code. |
| `message` | string | yes | Human-readable diagnostic. |
| `retry_after_ms` | integer | no | Retry delay for retryable conditions; must be at least `0`. |
| `job_id` | string | no | Job id when the error relates to an assigned job. |

Defined error and timeout cases include:

- `poll_timeout`: no assignment became available before the long-poll deadline;
  the worker should re-poll.
- `protocol_version_mismatch`: unsupported `protocol_version`.
- `malformed_message`: JSON or schema validation failed.
- `unknown_worker`: daemon has no active registration for `worker_id`.
- `capacity_exceeded`: worker or daemon rejects an assignment because capacity
  accounting is inconsistent.
- `heartbeat_missed`: daemon has declared a worker or job stale according to
  policy.
- `job_timeout`: job exceeded daemon execution policy and may be reclaimed.

## Assignment-scoped context capability

`fetch-context` is an implemented additive v1 capability. Older workers remain
compatible because they never send the new message, and older standard job
payload readers ignore `artifact_context`. A worker that does use the capability
must treat stable error codes as data and must not retry `not_authorized` or
`invalid_request` unchanged. `forge_unavailable` may be retried while the
assignment remains active.

## Versioning and compatibility

- `protocol_version` is a single integer in every message; v1 is `1`.
- A changed `protocol_version` signals a breaking change.
- Readers must ignore unknown fields in otherwise valid messages.
- Additive optional fields and message capabilities do not require a version bump.
- `artifact_context` is additive. The singular `artifact` remains valid and unchanged for backward compatibility.
- `fetch-context`/`context-response`, `activity-batch`/`activity-ack`, and `cancel-attempts` are optional additive v1 capabilities.
- Context requests and cancellation directives carry exact attempt identities. Compatibility-optional omitted ids compare only as `None` and never as wildcards.
- Rollout safety requires draining or restarting old workers that cannot consume `cancel-attempts` while deploying a daemon that may emit it; the additive v1 version alone cannot make those workers stop orphaned work.
- Additive optional typed model-failure and session-recovery evidence does not require a version bump; legacy failure objects remain valid.
- Context operations, cancellation causes, and public errors are closed vocabularies even though readers ignore unknown fields elsewhere.
