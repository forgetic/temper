# ADR 0024: Own agent activity in a shared versioned contract

## Status

Accepted

## Context

Temper needs one typed agent activity stream to feed worker spooling, engine
journals, operational projections, and the web timeline. Neither existing
process contract is a suitable owner:

- `temper-protocol-agent` owns the one-shot worker-to-agent context and terminal
  `WorkspaceResult`. Putting activity there would make engine and web consumers
  depend on the downstream process plane.
- `temper-protocol-worker` owns authenticated worker/daemon request and reply
  messages. Putting child frames there would make the agent depend on an
  upstream process plane it does not speak.

Logs are also not a contract. stdout, stderr, and `WorkspaceResult` have existing
process semantics and cannot safely become a live transcript transport.

Agent activity can contain source, prompts, assistant messages, tool arguments,
and tool results. It therefore needs a narrower privacy and size boundary than
an unrestricted logging or JSON-extension API.

## Decision

Create `temper-protocol-activity` as an implementation-independent leaf crate.
It contains versioned serde DTOs, closed event vocabularies, hard-bounded
content/blob shapes, classification helpers, and pure validation only. It has no
dependency on an agent, worker, engine, web, logging, or runtime crate.

### Trust boundary

`AgentActivityFrameV1` is untrusted child input. It contains only source
occurrence/elapsed timing, scope and parent-scope identity, optional turn
context, and typed event data. Unknown fields are rejected. In particular it has
no run ID, sequence, job, repository, artifact, role, action, correlation, or
agent-session identity. A child also cannot emit worker-owned run start or
terminal events.

The worker creates `AgentRunEventV1`. It stamps a new run ID and immutable
assignment identity from worker-owned context, optionally stamps the known agent
session ID, and assigns sequence numbers. Downstream components validate that
run, assignment, and session identity remain constant.

### Ordering and delivery

Sequence numbers are local to one run, begin at 1, and increase without gaps in
worker acceptance order across all scopes. The worker persists an accepted
canonical event before making it eligible for forwarding. A batch can begin at
any positive sequence because it may be a later slice, but its events must be
contiguous. A complete run stream must begin at 1.

Delivery is at least once. Consumers deduplicate by `(run_id, seq)` and return
only the highest durably accepted contiguous sequence. An acknowledgement does
not promise that a later, gapped event was accepted. Retransmission must carry
the same immutable identity and event content.

### Content and privacy

Event data is a tagged, closed enum. Message, delta, steering, and tool payloads
are bounded inline text or bounded content-addressed blob references; no
`serde_json::Value` or extension map is available. Blob attachments are base64
encoded, hard-size-limited, and checked against their SHA-256 reference.

Capture modes are `off`, `metadata`, `transcript`, and `diagnostic`. Thinking is
allowed only by an explicit diagnostic policy. Credentials, authorization
headers, environment values, provider tokens, unrestricted tool output, stdout,
stderr, and `WorkspaceResult` are excluded from every activity DTO. Later
producer and ingestion policy may be stricter than these absolute wire limits,
but may not relax them.

Required run/scope/turn/model/tool/usage/error/terminal boundaries and recorded
trace gaps are distinct from normal transcript records and droppable text or
thinking deltas. Trace capture or delivery failure must never alter the assigned
job result.

## Consequences

- Agent, worker, engine, config, and web code can share one vocabulary without
  importing another tier's implementation or process protocol.
- The worker remains the authority for assignment identity and ordering even
  when the child is compromised or crashes.
- Durable and live transports can retry safely and acknowledge only contiguous
  data.
- New event kinds or incompatible field changes require a protocol version
  decision and golden fixture updates rather than an unrestricted extension
  object.
- Large captured values incur explicit blob attachment and hashing work, which
  is accepted in exchange for deterministic bounds and reference integrity.
