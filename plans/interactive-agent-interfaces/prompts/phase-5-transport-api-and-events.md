# Phase 5 prompt — generic transport API and event seam

You are implementing Phase 5 of `plans/interactive-agent-interfaces/README.md`.
Assume Phases 1-4 are done.

## Read first

- `README.md`
- `AGENTS.md`
- `docs/reference/development-conventions.md`
- `docs/explanation/interactive-agent-interfaces.md`
- `docs/reference/interactive-conversation-interface.md`
- `docs/reference/product-manager-chat-api.md`
- `crates/temper-interaction/src/lib.rs` and sibling modules
- `crates/temper-production/src/product_chat_service.rs`
- `crates/temper-production/src/product_chat_repl.rs`
- `plans/interactive-agent-interfaces/README.md`

## Goal

Expose a generic transport-facing conversation API and event contract so external
frontends can connect to interactive agents without depending on product-manager
names. Keep product-manager endpoints as compatibility aliases.

## Tasks

1. Add a transport-neutral command/event model in `temper-interaction`, for
   example:

   - create/resume conversation;
   - send human turn;
   - receive agent reply;
   - list latest proposals;
   - accept proposal;
   - conversation events for transcript/proposal changes.

   The exact API can differ, but it must be profile-neutral.

2. Refactor the local HTTP service in `temper-production` so the core routing is
   generic. Suggested generic endpoints:

   ```text
   POST /conversations
   GET  /conversations/{id}
   POST /conversations/{id}/turns
   GET  /conversations/{id}/events      # SSE or documented future stream seam
   POST /conversations/{id}/proposals/{proposal_id}/accept
   ```

   Preserve the existing `/sessions` and `/drafts/{slug}/file` routes as
   product-manager compatibility aliases until a later deprecation decision.

3. Add an event/stream contract suitable for real-time clients.

   - If implementing SSE is small and safe, add it.
   - If SSE would make the phase too large, document the event model and expose a
     testable in-process event sink/source in `temper-interaction`; leave actual
     SSE as a follow-up prompt.

4. Update docs:

   - add/update the generic interaction API reference;
   - make `docs/reference/product-manager-chat-api.md` a product-manager profile
     specialization and alias note;
   - mention Matrix/web/mobile/voice adapters as external consumers of the
     generic API, not code this repo must ship.

5. Add tests for generic routing, alias routing, auth behavior, proposal
   acceptance idempotency, and event emission if implemented.

6. Update Phase 5 status in the plan README.

## Constraints

- Do not put realtime/chat methods on the `Forge` trait.
- Keep the service safe by default: loopback bind unless explicitly allowed, and
  bearer auth for non-loopback binds.
- Do not remove existing product-manager operator commands.
- Avoid broad web-framework dependencies unless justified; the current service is
  intentionally small.

## Validation

Run:

```sh
cargo fmt --all
cargo test -p temper-interaction
cargo test -p temper-production product_chat
cargo dev-clippy
cargo dev-check
```
