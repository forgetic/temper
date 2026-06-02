# Interactive agent interfaces — extraction plan

This plan fixes the current product-manager/chat confusion by making the generic
interactive-conversation layer explicit. The product-manager remains a useful
first profile, but it should be visibly *one consumer* of reusable Temper
primitives for connecting humans, chat systems, voice UIs, or other live
interfaces to agents.

Hand the prompt files to implementation agents **one phase at a time, in order**.
Each phase should land green and update this README's status.

## Responder boundary direction

The public extension boundary for interactive responders should be a
**process-level request/reply protocol**. A Rust `InteractiveResponder` trait
remains useful as an internal adapter interface, but external/profile
implementations should be able to run out of process by receiving a serialized
`ConversationRequest` and returning a serialized `ConversationReply` with inert
proposals. Temper keeps ownership of transcripts, proposal validation, explicit
acceptance, transport auth, and all Forge/workflow mutation.

## Problem statement

The current implementation already separates the product-manager from workflow
role agents in important ways:

- `ProductManagerAgent` is not a `temper_runner::Agent`.
- It has no Forge handles, no SDK tools, and no workflow mutation authority.
- The production product-chat layer persists transcripts and files intake issues
  only after an explicit human command.

But the reusable abstraction is still hidden behind product-manager names:

- `ProductManagerResponder`, `ProductChatSession`, and
  `temper-product-manager-chat` encode the first use case as the interface.
- `docs/reference/product-manager-chat-api.md` describes a product-manager API,
  not a framework conversation API with a product-manager profile.
- There is no generic transcript/session/responder/transport vocabulary for
  external chat, mobile, Matrix, web, or voice frontends to target.

## Goal

Temper should expose a small, explicit **interaction plane**:

```text
external chat / REPL / mobile / voice UI
        │ transport adapter: HTTP, SSE, Matrix, etc.
        ▼
interactive conversation runtime
        │ transcript store + turn dispatch + explicit proposal acceptance
        ▼
interactive agent profile
        │ one-turn responder; no workflow tools or Forge mutation
        ▼
Forge-backed transcript and normal workflow intake artifacts
```

The Forge remains the durable source of truth. Conversation services and
transports are restartable adapters; they may cache active sessions, but they do
not own workflow state.

## Terminology to make visible in code/docs

- **Interaction plane**: the non-workflow layer that connects live human-facing
  interfaces to agents and records durable transcript state.
- **Interactive agent profile**: a named behavior package for an interactive
  participant. `product-manager` is the first profile. A profile is not a
  workflow role unless a user explicitly models such a role in a workflow.
- **Interactive responder**: one-turn agent interface. It receives a transcript
  view and returns a reply plus optional proposals. It receives no Forge handles
  and performs no mutation.
- **Transcript store**: durable append-only-ish conversation persistence. The
  first implementation stores conversations as Forge issues plus comments.
- **Proposal**: an agent-suggested action that is inert until explicitly
  accepted. Product-manager draft intake issues are one proposal kind.
- **Transport adapter**: REPL, local HTTP/SSE, Matrix, web/PWA, Android, voice,
  etc. Transports translate live events into conversation turns and commands;
  they do not make workflow decisions.

## Non-goals

- Do not add chat/watch methods to the `Forge` trait. Forge remains a
  request/response collaboration contract; transport-specific realtime support
  is an adapter concern.
- Do not insert `product-manager` into reference-delivery workflow roles.
- Do not expose generic Forge mutation tools to interactive LLMs.
- Do not implement a rich web app, Android app, or voice stack in this repo.
- Do not make Matrix or SSE the only supported realtime shape; they are adapters.

## Target crate/module shape

Preferred new crate: `crates/temper-interaction/`.

The crate should stay provider-neutral and must not depend on `temper-agents`,
`temper-runner`, `temper-workflow`, `temper-production`, or provider SDK crates.
Phase 2's domain-only surface does not need `temper-forge`; later transcript or
acceptance modules may add it when public Forge identifiers become necessary.
Suggested modules:

- `types`: `ConversationId`, `ConversationProfileId`, `Participant`,
  `ConversationTurn`, `ConversationRequest`, `ConversationReply`, `Proposal`.
- `agent`: object-safe `InteractiveResponder` trait and responder error shape.
- `transcript`: `TranscriptStore` trait and Forge-backed transcript session.
- `proposal`: explicit proposal acceptance helpers, including issue-intake
  filing as one reusable implementation.
- `service`: in-memory active-session registry and transport-neutral commands.

`InteractiveResponder` is an in-process adapter trait, not the only extension
boundary. The preferred public boundary is a process responder protocol: Temper
sends a wire-serialized `ConversationRequest`, the responder returns one
wire-serialized `ConversationReply`, and Temper validates proposal ids/kinds
before persisting or accepting anything. A reusable process adapter may live in
`temper-interaction` if it stays provider-neutral, or in `temper-production` if
it is mostly deployable wiring; document the choice when implementing it.

Concrete pi-SDK responders, including the product-manager profile, should be
movable to a separate repository that consumes the process protocol. While the
current in-repo `temper-agents` implementation exists, it is a transitional
in-process implementation, not the required integration shape.

`temper-production` should contain deployable binaries and transport adapters,
not the core conversation abstractions or concrete provider SDK coupling.

## Compatibility rule

Keep the current dogfood/operator entry point working while extracting:

```sh
temper-product-manager-chat repl ...
temper-product-manager-chat serve ...
examples/dogfood/run.sh product-chat
```

It is acceptable for these commands to become thin wrappers around the generic
interaction runtime. Existing HTTP endpoints may remain as product-manager
aliases while generic names are introduced and documented.

## Phases

Status legend: ☐ pending · ☑ done

1. ☑ **Phase 1 — Document the interaction plane and boundary.**
   `prompts/phase-1-docs-and-boundary.md`

   Done: added canonical explanation/reference docs, marked product-manager chat
   as the first profile instance, and recorded lesson 0024 for the architectural
   steering. Validation run: `cargo fmt --all`; `rg` verified docs present
   product-manager as a profile/API instance rather than the framework
   abstraction. Markdown-only, so no Rust tests were required.

2. ☑ **Phase 2 — Add `temper-interaction` domain traits and types.**
   `prompts/phase-2-interaction-crate.md`

   Done: kept and tightened the interrupted domain-crate work after inspecting
   the uncommitted diff. The crate now provides provider-neutral conversation
   ids, participants, turns, wire-serializable requests/replies, inert proposals,
   issue-proposal payloads, an object-safe `InteractiveResponder`, deterministic
   proposal slug validation, duplicate-id checks, workspace/docs wiring, and
   hermetic tests including a process-boundary JSON round trip. Validation run:
   `cargo fmt --all`; `cargo test -p temper-interaction`;
   `cargo test -p temper-agents product_manager`;
   `cargo test -p temper-production product_chat`; `cargo dev-clippy`;
   `cargo dev-check`.

3. ☐ **Phase 3 — Extract Forge-backed transcript and issue-proposal core.**
   `prompts/phase-3-forge-transcript-core.md`

   Move the reusable pieces of `product_chat.rs` into `temper-interaction`:
   create/resume transcript issue, load recent turns, append human/agent turns,
   render/parse markers, and idempotently accept an issue-intake proposal.
   Product-manager-specific labels/prompts stay out of the generic code and come
   from profile config. Keep responder invocation abstract: session/runtime code
   may call an `InteractiveResponder` adapter, but it must not depend on
   `temper-agents`, pi SDKs, or an in-process product-manager implementation.

4. ☐ **Phase 4 — Add responder adapters and recast product-manager as a profile.**
   `prompts/phase-4-product-manager-profile.md`

   Add the process responder adapter/protocol and wire product-manager as one
   configured profile over the generic session/runtime. An in-process
   `ProductManagerAgent` adapter may remain as a transitional compatibility
   path, but the plan should not require concrete pi-SDK code to live in this
   repository. Product-specific draft semantics remain profile mapping; filing
   still happens only through explicit proposal acceptance.

5. ☐ **Phase 5 — Generalize the local transport API and realtime adapter seam.**
   `prompts/phase-5-transport-api-and-events.md`

   Introduce generic conversation endpoints and an event/stream contract suitable
   for web/PWA, Matrix, or voice adapters. Preserve existing product-manager API
   aliases. Transports talk only to the interaction service; they do not call the
   process responder directly and do not receive Forge mutation authority. Add
   docs showing product-manager as one configured profile and explaining where
   external frontends and external responder processes plug in.

## Follow-up extraction path

After this plan lands, a separate extraction task can move the pi-SDK-backed
product-manager/profile responder and any workflow-role LLM implementation into
an external repository. That external project should consume Temper's
process-responder protocol or, for tighter first-party builds, optionally expose
a Rust crate that implements `InteractiveResponder`/`temper_runner::Agent`. The
core Temper repository should remain usable with fake responders and process
adapters without depending on the pi SDK.

## Acceptance criteria

- The generic interactive-conversation API can be understood without knowing the
  product-manager use case.
- `product-manager` appears as a profile/example, not as the framework layer.
- `temper-forge`, `temper-workflow`, and `temper-runner` stay free of chat UI and
  LLM-provider dependencies; `temper-interaction` stays free of concrete LLM
  provider/SDK dependencies.
- Interactive responder requests/replies/proposals are wire-serializable for a
  process boundary.
- Interactive responders cannot mutate Forge or workflow state directly.
- Proposal acceptance is explicit and idempotent.
- Existing product-manager dogfood commands and tests remain green.
- New docs answer: "How do I connect a chat-like frontend to an agent?", "How do
  I plug in an external responder process?", and "How is that different from a
  workflow role agent?"

## Validation expectations for code phases

Each implementation phase should run at least:

```sh
cargo fmt --all
cargo test -p temper-interaction
cargo test -p temper-agents product_manager
cargo test -p temper-production product_chat
cargo dev-clippy
cargo dev-check
```

Narrow the test list only when the phase has not touched the corresponding
crate, and state the reason in the handoff.

## Relevant starting points

- `crates/temper-agents/src/product_manager.rs`
- `crates/temper-agents/src/prompts/product_manager.md`
- `crates/temper-production/src/product_chat.rs`
- `crates/temper-production/src/product_chat_service.rs`
- `crates/temper-production/src/product_chat_repl.rs`
- `docs/reference/product-manager-chat-api.md`
- `docs/reference/llm-agents.md`
- `docs/explanation/agentic-workflows.md`
- `plans/product-manager-chat/README.md`
