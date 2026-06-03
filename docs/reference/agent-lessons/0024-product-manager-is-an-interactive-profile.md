# Lesson 0024: Treat product-manager as an interactive profile

## Tags

`architecture`, `agents`, `workflow`, `product-chat`, `interaction`

## Trigger

Human steering clarified that the product-manager chat path is the first use of
a broader interactive-conversation plane, not the framework abstraction itself.

## What went wrong

The implemented and documented surfaces were named around product-manager chat,
which made the first profile look like the reusable interface. That could lead
future agents to insert `product-manager` into workflow roles, expose Forge
mutation authority to interactive LLMs, or make transports appear to own durable
state.

## Steering for future agents

Model the reusable layer as the interaction plane: profiles provide behavior,
responders return replies and inert proposals, transcripts remain durable in the
Forge, and transports are adapters. Product-manager is a profile/example unless a
user explicitly defines a separate workflow role. Non-test/non-fixture `crates/`
sources should not mention concrete profile ids, compatibility DTOs, profile
routes, or command aliases; use the source grep guard when touching interaction
runtime code.

## Where this is now documented

- `docs/explanation/interactive-agent-interfaces.md`
- `docs/reference/interactive-conversation-interface.md`
- `examples/dogfood/README.md`
- `plans/interactive-agent-interfaces/README.md`
