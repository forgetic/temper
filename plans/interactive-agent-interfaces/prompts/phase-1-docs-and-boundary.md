# Phase 1 prompt — document the interaction plane and boundary

You are implementing Phase 1 of `plans/interactive-agent-interfaces/README.md`.
Do not start later phases.

## Read first

- `README.md`
- `AGENTS.md`
- `docs/how-to/start-a-development-session.md`
- `docs/reference/development-conventions.md`
- `docs/reference/llm-agents.md`
- `docs/reference/product-manager-chat-api.md`
- `docs/explanation/agentic-workflows.md`
- `plans/product-manager-chat/README.md`
- `plans/interactive-agent-interfaces/README.md`

## Goal

Create the canonical documentation distinction:

- Temper provides reusable interactive-conversation primitives/interfaces.
- Product-manager is the first profile/example that uses those primitives.
- Interactive profiles are not workflow roles by default.
- Transports are adapters; Forge remains durable truth.

## Tasks

1. Add a focused explanation doc, suggested path:
   `docs/explanation/interactive-agent-interfaces.md`.

   It should define the interaction plane, profile, responder, transcript store,
   proposal, and transport adapter. Keep it conceptual and short.

2. Add or update a reference doc for the contract, suggested path:
   `docs/reference/interactive-conversation-interface.md`.

   It should describe the intended generic API/traits, invariants, and authority
   boundaries without over-specifying implementation details that Phase 2 has not
   built yet.

3. Update `docs/reference/product-manager-chat-api.md` so it clearly says:

   - this API is currently the product-manager profile instance of the broader
     interaction-plane idea;
   - external web/mobile/Matrix/voice frontends should eventually target the
     generic interaction API;
   - product-manager draft issues are one proposal type, filed only after
     explicit acceptance.

4. Update `docs/README.md` and `docs/reference/README.md` / explanation index if
   needed so future agents can find the new docs.

5. If the work reveals durable steering, promote it directly into the relevant
   canonical doc, test, or ADR.

6. Update the status line for Phase 1 in
   `plans/interactive-agent-interfaces/README.md` to `☑ done`, with a compact
   validation note.

## Constraints

- Do not change Rust code in this phase unless a doc link requires a tiny module
  comment adjustment.
- Do not rename product-manager binaries or endpoints yet.
- Do not claim the generic interface exists in code yet; phrase it as the target
  contract until Phase 2 lands.

## Validation

Run at least:

```sh
cargo fmt --all
```

If you only changed markdown, no Rust test run is required; say so in the
handoff. Use `rg` to verify the docs no longer present product-manager as the
framework abstraction.
