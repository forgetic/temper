# Lesson 0025: Prefer process boundaries for interactive responders

## Tags

`architecture`, `agents`, `interaction`, `process-boundary`, `pi-sdk`

## Trigger

While Phase 2 of the interactive-agent-interfaces plan was in progress, human
steering favored a process-level responder boundary over requiring concrete
interactive agent implementations to be Rust crates in this repository.

## What went wrong

The earlier phase wording made it too easy to treat the Rust
`InteractiveResponder` trait as the public extension boundary and to assume the
pi-SDK product-manager implementation must remain in-process. That would keep
provider auth, SDK version churn, and concrete profile behavior coupled to the
core Temper repository.

## Steering for future agents

Keep `temper-interaction` as the owner of provider-neutral request/reply,
transcript, proposal, and acceptance contracts. Use the Rust responder trait as
an adapter interface, but make the public extension shape a process protocol:
Temper sends a serialized conversation request and receives one serialized reply
with inert proposals. Temper still owns validation, transcript persistence,
explicit acceptance, and all Forge/workflow mutation.

## Where this is now documented

- `plans/interactive-agent-interfaces/README.md`
- `plans/interactive-agent-interfaces/prompts/phase-4-product-manager-profile.md`
- `docs/reference/interactive-conversation-interface.md`
- `docs/reference/interactive-process-responder-protocol.md`
