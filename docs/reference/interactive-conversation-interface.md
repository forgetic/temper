# Interactive conversation interface

This page defines the target contract for Temper's generic interaction plane.
The current implementation is still product-manager-specific; later phases will
extract code that follows this contract. Treat names here as intended API roles,
not as a promise that exact Rust items already exist.

## Scope

The interface supports live human-to-agent conversations whose durable record is
stored in the Forge. It is transport-neutral, profile-neutral, LLM-provider
agnostic, and separate from workflow role execution.

It does not add realtime methods to the `Forge` trait, expose broad Forge tools
to responders, or make chat transports authoritative for workflow state.

## Intended API roles

- **Conversation identifiers and participants**: typed ids for conversations,
  profiles, turns, participants, and proposals. Public APIs should avoid raw
  string status values when the domain has a fixed meaning.
- **Interactive profile**: configuration that names the responder behavior,
  transcript policy, proposal kinds, and acceptance rules for one conversational
  use case. `product-manager` is one profile instance.
- **Interactive responder**: an object-safe one-turn interface. Given an
  immutable request containing the profile id, transcript view, latest human
  turn, and profile context, it returns a reply and zero or more proposals. It
  receives no Forge handle and cannot perform mutations.
- **Transcript store**: creates or resumes conversations, appends ordered turns,
  and reads a transcript view suitable for a responder. A Forge-backed store may
  use issues and comments; any in-memory session registry is only a cache.
- **Proposal**: a typed, serializable suggested action with stable identity,
  display text, and profile-specific payload. Proposals are inert until accepted.
- **Proposal acceptance**: a narrow command that records explicit acceptance,
  reloads current durable state, validates the proposal against profile policy,
  and applies the allowed mutation idempotently.
- **Interaction service**: transport-neutral orchestration for create/resume,
  append human turn, run responder, persist reply/proposals, list current
  proposals, and accept a proposal.
- **Transport adapter**: maps REPL, HTTP/SSE, Matrix, web/mobile, or voice events
  to service commands and renders responses. It owns protocol details only.

## Invariants

- The Forge-backed transcript is the durable conversation record.
- Responders are pure with respect to Forge and workflow state: they produce
  replies and proposals, not mutations.
- A proposal is never applied without explicit human acceptance.
- Proposal acceptance is idempotent and uses stable correlation markers where it
  creates Forge artifacts.
- Acceptance reloads current Forge state before mutating, so stale transport or
  session-cache data cannot authorize changes.
- Interactive profiles are not workflow roles by default and are not inserted
  into workflow queues unless the user explicitly defines such a workflow role.
- Transports are replaceable adapters; losing a transport process must not lose
  accepted workflow state or transcript history.
- Secrets and provider credentials stay out of transcripts and API responses.

## Authority boundaries

`temper-forge` remains a request/response collaboration contract. It stores and
retrieves the artifacts used by transcript stores and proposal acceptance, but it
should not grow chat transport APIs.

`temper-workflow` and `temper-runner` remain responsible for workflow queues,
transitions, role tools, gates, and recovery. The interaction plane may create a
normal intake issue after acceptance, but the workflow runtime decides what that
issue means next.

`temper-agents` may implement concrete responders for profiles. Provider SDKs
and prompts stay there, not in the generic interaction contract.

`temper-production` may host deployable binaries and adapters, including the
existing product-manager commands. Those commands can become compatibility
wrappers over the generic interaction service once the implementation lands.
