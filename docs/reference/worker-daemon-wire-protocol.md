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
`JobContext.allowed_verdicts`, `JobResult.verdict`, `JobResult.body`, and
`JobResult.children` are all optional, and the protocol version remains `1`.

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
| `base_branch` | string | no | Workspace base branch for checkout and implementation PR target. Defaults to the normalized Forge default branch, but workflow metadata `target_branch` may override it for issue-backed implementation work. |
| `branch_hint` | string | no | Deterministic worker branch suggestion, for example `agent/pr-for-code-42`. |
| `correlation_key` | string | no | Deterministic PR correlation key, for example `pr-for-code-42`. |
| `artifact` | object | no | Enqueue-time issue snapshot. Omitted for older minimal payloads and for PR-targeted jobs in v1. |
| `artifact.number` | integer | yes when `artifact` is present | Repository-scoped issue number. |
| `artifact.title` | string | yes when `artifact` is present | Issue title at enqueue time. |
| `artifact.body` | string | yes when `artifact` is present | Issue body at enqueue time. |
| `artifact.labels` | array of strings | yes when `artifact` is present | Issue labels at enqueue time. |
| `artifact.state` | string | yes when `artifact` is present | Debug-formatted issue state, for example `Open`. |
| `action` | string | no | Workflow action (intent-level tool / transition id) this job services, for example `open_pr` or `triage_intake`. |
| `checkout_capability` | string | no | Checkout capability the worker should prepare: `writable`, `read_only`, `pull_request_read_only`, or `pull_request_writable`. Absent means writable, preserving v1's original behavior. |
| `allowed_verdicts` | array of strings | no | Verdict vocabulary declared by `action`'s `outcomes` keys, in deterministic order. Empty or absent for a plain coding job. |

For compatibility, old minimal payloads containing only `role`, `repo`, `queue`,
and `artifact_kind` remain valid; the enrichment fields are optional. The
`action`, `checkout_capability`, and `allowed_verdicts` additions are also
optional, and adding them does not change the protocol version: it remains `1`.
Readers must ignore unknown fields in the standard payload just as they do for
protocol messages.

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
| `body` | string | no | Authored body accompanying a verdict, such as a rewritten issue spec or PR review body. |
| `children` | array of objects | no | Workspace-authored child issues for breakdown verdicts such as `needs_breakdown`. Empty or absent means no children. |
| `children[].slug` | string | yes | Stable per-child identifier within the result; seeds the child's correlation key and is referenced by sibling `depends_on` entries. |
| `children[].title` | string | yes | Child issue title. |
| `children[].body` | string | yes | Child issue body. |
| `children[].labels` | array of strings | no | Labels to apply when creating the child issue. |
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

Verdict jobs are successful jobs whose result may carry `verdict` plus `body`
or breakdown `children` and no `branch`; the allowed vocabulary comes from the
assignment payload's `allowed_verdicts`. The daemon binds `children` only when
the routed verdict transition declares a `create_issues` effect; a child's
optional `target_repo` uses the same `owner/name` shape as daemon `--repo` and
omits to the assignment's repository. These result fields are optional for
backward compatibility, and their addition does not change the protocol version:
it remains `1`.

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

## Reserved future capability

`fetch_context` is reserved as a future worker-to-daemon message for
daemon-mediated read-only Forge reads during a job. It is intentionally not part
of the v1 message union. The compatibility rules below must allow adding it
later without a breaking version bump when it is additive and optional for older
peers.

## Versioning and compatibility

- `protocol_version` is a single integer in every message; v1 is `1`.
- A changed `protocol_version` signals a breaking change.
- Readers must ignore unknown fields in otherwise valid messages.
- Additive optional fields do not require a version bump.
- Future additive capabilities such as `labels`-based routing or `fetch_context`
  must not require Smith workers to depend on Temper internals.
- This rule is load-bearing for the zero-dependency consolidation goal.
