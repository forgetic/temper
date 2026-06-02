# Phase 4 prompt — responder adapters and product-manager profile wiring

You are implementing Phase 4 of `plans/interactive-agent-interfaces/README.md`.
Assume Phases 1-3 are done.

## Read first

- `README.md`
- `AGENTS.md`
- `docs/reference/development-conventions.md`
- `docs/explanation/interactive-agent-interfaces.md`
- `docs/reference/interactive-conversation-interface.md`
- `docs/reference/llm-agents.md`
- `crates/temper-interaction/src/lib.rs` and sibling modules
- `crates/temper-agents/src/product_manager.rs`
- `crates/temper-agents/src/prompts/product_manager.md`
- `crates/temper-production/src/product_chat.rs`
- `crates/temper-production/src/product_chat_repl.rs`
- `crates/temper-production/src/product_chat_service.rs`
- `plans/interactive-agent-interfaces/README.md`

## Goal

Make product-manager visibly one interactive profile layered on the generic
interaction runtime, and add the responder integration shape that lets concrete
profile implementations live out of process. The Rust `InteractiveResponder`
trait remains a useful in-process adapter; the preferred public extension
boundary is a process that receives a serialized `ConversationRequest` and
returns one serialized `ConversationReply`.

## Tasks

1. Add or wire a generic process responder adapter/protocol.

   Suggested behavior:

   - configured command plus optional working directory/env allow-list;
   - send one JSON `ConversationRequest` to stdin, or another documented simple
     request channel if you justify it;
   - read exactly one JSON `ConversationReply` from stdout;
   - validate proposal ids/kinds with the generic interaction helpers before the
     reply is persisted or exposed;
   - enforce timeout, nonzero-exit, malformed-JSON, and duplicate-proposal error
     handling;
   - never pass Forge tokens, broad Forge handles, or workflow tools to the
     process.

   The adapter may live in `temper-interaction` if it remains provider-neutral,
   or in `temper-production` if the implementation is mostly deployable process
   wiring. Document the choice. Do not couple it to pi SDK types.

2. Keep the in-process responder path available.

   If the in-repo `ProductManagerAgent` still exists, implement
   `InteractiveResponder` for it directly or through a thin adapter. Treat that
   as transitional compatibility, not as the required public integration model.

3. Refactor product-chat production modules into thin profile wiring over the
   generic session/runtime from Phase 3:

   - profile config: label `product`, intake label `untriaged`, title prefix,
     marker namespace/profile id;
   - responder: process responder when configured; in-process product-manager
     fallback only while this repository still carries that implementation;
   - transport: existing REPL and service commands.

4. Decide whether `ProductManagerDraftIssue` remains a product-specific type or
   becomes a type alias/wrapper around a generic `IssueProposal`. Prefer the
   least disruptive path, but ensure generic code no longer depends on product
   names.

5. Keep the binary and dogfood wrapper stable:

   ```sh
   temper-product-manager-chat repl ...
   temper-product-manager-chat serve ...
   ./examples/dogfood/run.sh product-chat
   ```

   If you add CLI/env configuration for the external responder command, keep the
   existing default behavior working unless the operator explicitly selects the
   process responder.

6. Update doc comments and public docs so product-manager is consistently called
   an interactive profile/example, not the abstraction itself. Document the
   process responder protocol sufficiently for an external repo to implement it.

7. Update Phase 4 status in the plan README.

## Constraints

- Do not add checked-in workflow-role prompts.
- Do not make product-manager a `temper_runner::Agent`.
- Do not allow product-manager or any process responder to file issues from
  inside the responder; filing remains explicit in the interaction/proposal
  layer.
- Do not add pi SDK, provider auth, or `temper-agents` dependencies to
  `temper-interaction`.
- Do not move pi-SDK code to a separate repository in this phase unless the task
  explicitly expands scope; just make that extraction possible.
- Keep compatibility tests for product-chat behavior.

## Validation

Run:

```sh
cargo fmt --all
cargo test -p temper-interaction
cargo test -p temper-agents product_manager
cargo test -p temper-production product_chat
cargo dev-clippy
cargo dev-check
```
