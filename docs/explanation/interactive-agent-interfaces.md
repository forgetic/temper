# Interactive agent interfaces

Interactive agent interfaces are Temper's non-workflow path for connecting live
human-facing tools to agents while still recording durable state in the Forge.
The current product-manager chat service is the first concrete profile using
this idea, but it is not the abstraction itself.

The generic interaction-plane contract now lives in `temper-interaction` for
responder requests, replies, participants, inert proposals, Forge-backed
transcripts/sessions, explicit idempotent issue-proposal acceptance,
transport-facing commands/events, a replayable in-process event seam, and the
provider-neutral process responder adapter. The local product-manager HTTP
service now exposes generic `/conversations` routes while keeping the historical
product-manager aliases.

## Shape

```text
web / REPL / Matrix / mobile / voice
        │
        ▼
transport adapter
        │ live turns, event replay, and explicit commands
        ▼
interaction plane
        │ transcript store + responder dispatch + proposal acceptance
        ├──────────────► interactive responder process
        │                  serialized request/reply, inert proposals only
        ▼
Forge transcript and normal workflow artifacts
```

The Forge remains the durable source of truth. Services may keep active-session
caches for latency, transports may keep UI state, and responder processes may be
restarted independently, but all of them can be rebuilt or retried from
Forge-backed transcripts and accepted workflow artifacts.

## Vocabulary

- **Interaction plane**: the layer that coordinates conversational turns,
  transcript persistence, responder dispatch, and explicit proposal acceptance.
  It is separate from the workflow runtime.
- **Interactive agent profile**: a named behavior package for a conversational
  participant. `product-manager` is the first profile. A profile is not a
  workflow role unless a user explicitly models it as one in a workflow.
- **Interactive responder**: the one-turn agent interface. It receives a
  transcript view and profile context, then returns a reply plus optional
  proposals. It receives no Forge handles and does not mutate workflow state.
  A Rust trait can adapt in-process responders, but the preferred public
  extension boundary is the documented process protocol that exchanges
  serialized requests/replies.
- **Transcript store**: durable conversation persistence. The first backing
  store uses Forge issues and comments; transports should treat it as the
  recoverable record, not as a UI cache.
- **Proposal**: an agent-suggested action that is inert until a human explicitly
  accepts it. Product-manager draft intake issues are one proposal type.
- **Transport adapter**: REPL, HTTP/SSE, Matrix, web/PWA, Android, or voice
  integration that turns live events into conversation turns and acceptance
  commands. Transports do not make workflow decisions.

## Boundary with workflows

Workflow role agents operate on queues, transitions, leases, and gates compiled
from a workflow definition. Interactive profiles answer live conversation turns.
They may propose work, but they do not directly claim workflow authority.

When an accepted proposal creates or updates a workflow artifact, that mutation
is performed by the interaction runtime through a narrow, idempotent acceptance
path. The current issue-intake path records a hidden marker, transcript backlink,
and requested-by field when available. The resulting issue is then ordinary
Forge state for the workflow runtime to observe.

This keeps the human conversation interface reusable without adding chat, mobile,
voice, or concrete LLM SDK concerns to `temper-forge`, `temper-workflow`, or
`temper-runner`.
