# Phase 4 prompt — recast product-manager as an interactive profile

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

Make product-manager visibly a profile/example layered on the generic
interaction interface, while preserving current operator commands.

## Tasks

1. In `temper-agents`, adapt `ProductManagerAgent` to the generic
   `InteractiveResponder` trait.

   Options:

   - implement the trait directly and map generic `ConversationRequest` /
     `ConversationReply` to the existing product-manager prompt JSON; or
   - add a tiny adapter type if keeping the existing typed request/response is
     cleaner.

   Either way, keep the invariant: no Forge handles, no SDK tools, no workflow
   mutation.

2. Decide whether `ProductManagerDraftIssue` remains a product-specific type or
   becomes a type alias/wrapper around a generic `IssueProposal`. Prefer the
   least disruptive path, but ensure generic code no longer depends on product
   names.

3. Refactor `temper-production` product-chat modules into thin wiring:

   - profile config: label `product`, intake label `untriaged`, title prefix,
     marker namespace/profile id;
   - responder: `ProductManagerAgent` through the generic trait;
   - transport: existing REPL and service commands.

4. Keep the binary and dogfood wrapper stable:

   ```sh
   temper-product-manager-chat repl ...
   temper-product-manager-chat serve ...
   ./examples/dogfood/run.sh product-chat
   ```

5. Update doc comments and public docs so product-manager is consistently called
   an interactive profile/example, not the abstraction itself.

6. Update Phase 4 status in the plan README.

## Constraints

- Do not add checked-in workflow-role prompts.
- Do not make product-manager a `temper_runner::Agent`.
- Do not allow product-manager to file issues from inside the responder; filing
  remains explicit in the interaction/proposal layer.
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
