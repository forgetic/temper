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
`JobContext.source_metadata`, `JobContext.artifact_context`, `JobResult.verdict`, `JobResult.body`,
`JobResult.children`, and `JobResult.children[].kind` are all optional, and the
protocol version remains `1`. The `fetch-context`/`context-response` pair is an
additive assignment-scoped capability in the same v1 envelope.

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
| `job_id` | string | yes | Daemon-generated unique job id. |
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
| `artifact_context` | object | no | Versioned bounded graph bundle containing the primary snapshot, mandatory ancestry, compact related indexes, diagnostics, and explicit truncation flags. Additive; legacy workers may ignore it. |
| `artifact_context.version` | integer | yes when `artifact_context` is present | Artifact-context schema version, currently `1` and independent of the worker protocol version. |
| `artifact_context.repository` | object | yes when `artifact_context` is present | Stable id and configured `owner/name` path of the coordinating repository. |
| `artifact_context.artifact_type` | string | yes when `artifact_context` is present | `issue` or `pull_request`. |
| `artifact_context.snapshots` | array | no | Full bounded artifact records. The primary snapshot is first; mandatory lineage follows in deterministic root-first order. |
| `artifact_context.index` | array | no | Deterministically ordered compact records, with `snapshot_index` when full content is present. |
| `artifact_context.relations` | array | no | Directed `parent`, `dependency`, or `related` edges. |
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
| `verdict_contracts` | object | no | Workflow-derived result requirements keyed by verdict: child cardinality/kinds, required child/source metadata, and required PR text or authored body. Required child metadata must appear non-blank in each child body's `<!-- temper:workflow ... -->` JSON block. |
| `source_metadata` | object | no | Parsed assignment-time source metadata used by worker/agent preflight validation. The engine re-reads current Forge state before mutation. |

For compatibility, old minimal payloads containing only `role`, `repo`, `queue`,
and `artifact_kind` remain valid; the enrichment fields are optional. The
`action`, `checkout_capability`, `allowed_verdicts`, `verdict_contracts`, and
`source_metadata`, and `artifact_context` additions are also optional, and adding them does not change the protocol version: it remains `1`.
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
| `operation` | object | yes | Exactly one closed-vocabulary operation described below. |

`forge_get_item` accepts `repo`, positive `number`, optional `type`
(`issue`/`pull_request`), and `include_comments` (default `false`).
`forge_list_related` accepts `repo`, positive `number`, optional `type`, a
non-empty unique subset of `parent`, `child`, `dependency`, `dependent`,
`produced_pr`, `body_reference`, and `referenced_by`, plus optional bounded
`depth` and `limit`. Repeated calls are supported so a client can deliberately
follow indirect relations without one unbounded graph request.

The daemon authorizes the worker-pool credential, exact `(worker_id, job_id)`
active-assignment binding, and configured repository before any Forge read.
Pending, completed, another worker's, and unconfigured-repository reads are
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
| `jobs[].state` | string | yes | Job state: `running`, `waiting`, or `finishing`. |
| `jobs[].message` | string | yes | Short human-readable progress text. |
| `free_capacity` | integer | no | Current free capacity; must be at least `0` when present. |

Heartbeat interval and missed-heartbeat threshold are deployment-configured
daemon policy, not fixed wire constants. If a worker misses the threshold, the
daemon may mark the worker unhealthy, reclaim leases, and reassign eligible work.

### `result` — worker → daemon

Worker returns the structured result for one assigned job.

| Field | Type | Required | Semantics |
| --- | --- | --- | --- |
| `protocol_version` | integer | yes | Constant `1`. |
| `type` | string | yes | Constant `result`. |
| `worker_id` | string | yes | Worker id. |
| `job_id` | string | yes | Assigned job id. |
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
| `summary` | string | no | Short result summary suitable for logs or operator display. |
| `details` | object | no | Arbitrary structured role-specific result details. |

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
| `disposition` | string | yes | `accepted`, `superseded`, or `reclaimed`. |
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
- `fetch-context`/`context-response` are optional v1 capabilities; workers that do not use them continue to interoperate.
- Context operations and public errors are closed vocabularies even though readers ignore unknown fields elsewhere.
