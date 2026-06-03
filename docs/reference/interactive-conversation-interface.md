# Interactive conversation interface

This page defines the target contract for Temper's generic interaction plane.
`crates/temper-interaction` contains the reusable core of this contract:
typed conversation/profile/proposal ids, participants, turns, wire-serializable
requests/replies, inert proposals, deterministic proposal-id validation,
user-defined interaction profile spec validation, the object-safe responder
adapter trait, the provider-neutral process responder adapter, Forge-backed
transcript/session helpers, explicit idempotent issue-proposal acceptance, and
transport-facing command/event types with a small in-process event log for
adapters. See [Interaction profile spec](interaction-profile-spec.md) for the
raw-to-validated profile contract.

## Scope

The interface supports live human-to-agent conversations whose durable record is
stored in the Forge. It is transport-neutral, profile-neutral, LLM-provider
agnostic, and separate from workflow role execution.

It does not add realtime methods to the `Forge` trait, expose broad Forge tools
to responders, require responders to run in-process, or make chat transports
authoritative for workflow state.

## Domain crate boundary

`temper-interaction` should remain provider-neutral and have no dependency on
external responder crates, `temper-runner`, `temper-workflow`,
`temper-production`, or any LLM SDK. It may use the portable `temper-forge`
trait for transcript and acceptance storage. Responders still receive no Forge
handle, and proposals are
data until the interaction runtime's acceptance path acts on them.

The Rust trait is an adapter interface. The preferred public extension boundary
is the process protocol using the same serialized request/reply types; see
[Interactive process responder protocol](interactive-process-responder-protocol.md).

## Intended API roles

- **Conversation identifiers and participants**: typed ids for conversations,
  profiles, turns, and proposals. Public APIs should avoid raw string status
  values when the domain has a fixed meaning.
- **Interactive profile**: validated configuration that names the responder
  behavior, transcript policy, proposal kinds, commands, and acceptance rules for
  one conversational use case. `product-manager` is one fixture/profile instance,
  not a production constant.
- **Interactive responder**: a one-turn interface. Given an immutable request
  containing the profile id, transcript view, latest human turn, and profile
  context, it returns a reply and zero or more proposals. It receives no Forge
  handle and cannot perform mutations. In Rust this is an object-safe trait; for
  external implementations the same contract should be exposed as a process
  request/reply protocol.
- **Transcript store**: creates or resumes conversations, appends ordered turns,
  and reads a transcript view suitable for a responder. The current Forge-backed
  store uses transcript issues plus comments; any in-memory session registry is
  only a cache.
- **Proposal**: a typed, serializable suggested action with stable identity,
  display text, and profile-specific payload. Proposals are inert until accepted.
- **Proposal acceptance**: a narrow command that records explicit acceptance,
  reloads current durable state, validates the proposal against profile policy,
  and applies the allowed mutation idempotently. Issue-intake acceptance searches
  for a hidden marker before creating a labeled workflow intake issue.
- **Process responder adapter**: provider-neutral glue in `temper-interaction`
  that invokes an external command with a serialized `ConversationRequest`,
  reads one serialized `ConversationReply`, enforces timeout/exit/parse errors,
  clears ambient env except allow-listed names, and validates proposal ids/kinds
  plus built-in issue payloads before the interaction service persists anything.
- **Interaction service**: transport-neutral orchestration for create/resume,
  append human turn, run a responder adapter, persist reply/proposals, cache
  current proposals, emit conversation events, and accept a proposal.
- **Transport adapter**: maps REPL, HTTP/SSE, Matrix, web/mobile, or voice events
  to service commands and renders responses. It owns protocol details only.

## Transport command and event contract

`temper-interaction::transport` defines profile-neutral commands and DTOs for:

- opening a conversation, optionally resuming a transcript issue;
- sending one human turn and receiving one `ConversationReply`;
- listing latest inert proposals;
- accepting a proposal by stable `ProposalId`;
- replaying conversation events for transcript and proposal changes.

The production local HTTP adapter currently exposes one configured profile with
these generic routes:

```text
POST /conversations
GET  /conversations/{id}
POST /conversations/{id}/turns
GET  /conversations/{id}/proposals
GET  /conversations/{id}/events
POST /conversations/{id}/proposals/{proposal_id}/accept
```

`POST /conversations` accepts an optional `profile_id` and optional
`transcript_issue`; single-profile deployments reject any unconfigured profile.
`POST /conversations/{id}/turns` accepts `{ "body": "..." }` and returns a
reply object plus the latest proposals. Event replay returns JSON events with a
monotonic in-process sequence, `kind`, timestamp, conversation id, and typed
payload. SSE is not yet enabled by the local adapter; `GET .../events` returns a
snapshot with `streaming:false` so web, Matrix, mobile, and voice adapters can be
written against the event schema before a streaming implementation lands.

## Invariants

- The Forge-backed transcript is the durable conversation record.
- Responders are pure with respect to Forge and workflow state: they produce
  replies and proposals, not mutations.
- Responder requests and replies remain JSON-serializable so an implementation
  can run out of process without weakening Temper's authority boundary.
- A proposal is never applied without explicit human acceptance.
- Proposal acceptance is idempotent and uses stable correlation markers where it
  creates Forge artifacts.
- Acceptance reloads current Forge state before mutating, so stale transport or
  session-cache data cannot authorize changes.
- Interactive profiles are not workflow roles by default and are not inserted
  into workflow queues unless the user explicitly defines such a workflow role.
- Transports are replaceable adapters; losing a transport process must not lose
  accepted workflow state or transcript history.
- Losing or restarting an external responder process must not lose transcript
  history or accepted workflow state.
- Secrets and provider credentials stay out of transcripts and API responses.

## Authority boundaries

`temper-forge` remains a request/response collaboration contract. It stores and
retrieves the artifacts used by transcript stores and proposal acceptance, but it
should not grow chat transport APIs.

`temper-workflow` and `temper-runner` remain responsible for workflow queues,
transitions, role tools, gates, and recovery. The interaction plane may create a
normal intake issue after acceptance, but the workflow runtime decides what that
issue means next.

Concrete responder implementations live outside Temper behind the
[interactive process responder protocol](interactive-process-responder-protocol.md).
Provider SDKs, auth files, and prompts stay out of the generic interaction
contract; Smith is the reference pi-SDK-backed product-manager responder.

`temper-production` hosts deployable binaries and adapters, including the
existing product-manager commands. Those commands are compatibility wrappers over
the generic interaction session/runtime while the generic transport API is still
being introduced.
