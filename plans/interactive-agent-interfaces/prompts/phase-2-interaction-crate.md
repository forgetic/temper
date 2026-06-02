# Phase 2 prompt — add `temper-interaction` domain traits and types

You are implementing Phase 2 of `plans/interactive-agent-interfaces/README.md`.
Assume Phase 1 is done. Do not start extraction from product-chat yet except for
small compile wiring needed by the new crate.

## Read first

- `README.md`
- `AGENTS.md`
- `docs/how-to/start-a-development-session.md`
- `docs/reference/development-conventions.md`
- `docs/reference/agent-lessons/README.md`
- `docs/explanation/interactive-agent-interfaces.md`
- `docs/reference/interactive-conversation-interface.md`
- `docs/reference/llm-agents.md`
- `crates/temper-agents/src/product_manager.rs`
- `crates/temper-production/src/product_chat.rs`
- `plans/interactive-agent-interfaces/README.md`

## Goal

Introduce a provider-neutral crate for interactive conversation primitives so the
framework interface is no longer named after product-manager.

## Tasks

1. Add `crates/temper-interaction/` to the workspace.

2. Keep dependencies minimal:

   - may depend on `async-trait`, `serde`, `serde_json`, `thiserror`/std error if
     already appropriate, and `temper-forge` only if the public types need Forge
     identifiers in this phase;
   - must not depend on `temper-agents`, `temper-runner`, `temper-workflow`,
     `temper-production`, or the pi SDK.

3. Define generic domain types. Suggested vocabulary:

   - `ConversationProfileId`
   - `ConversationId`
   - `Participant` / `ParticipantKind`
   - `ConversationTurn`
   - `ConversationRequest`
   - `ConversationReply`
   - `Proposal` / `ProposalKind` / `ProposalId`

   Keep names product-neutral. If you need a draft issue shape, call it an
   `IssueProposal` or similar, not `ProductManagerDraftIssue`.

4. Define an object-safe responder trait, for example:

   ```rust
   #[async_trait]
   pub trait InteractiveResponder: Send + Sync {
       async fn respond(
           &self,
           request: &ConversationRequest,
       ) -> Result<ConversationReply, InteractionError>;
   }
   ```

   The exact error shape is up to you, but it must be able to wrap/adapt
   profile-specific/provider errors in later phases.

5. Add validation helpers for deterministic proposal ids/slugs, adapting the
   existing product-manager slug rule if appropriate.

6. Add hermetic unit tests for serialization, validation, duplicate proposal ids,
   and trait object usability where possible.

7. Update workspace `Cargo.toml`, docs, and Phase 2 status in the plan README.

## Constraints

- Do not move product-manager code yet.
- Do not alter the public product-manager API unless unavoidable.
- Keep source files under 600 lines.
- Avoid over-modeling transport details in this phase; transports come later.

## Validation

Run:

```sh
cargo fmt --all
cargo test -p temper-interaction
cargo dev-clippy
cargo dev-check
```

If adding the crate requires lockfile changes, ensure they are intentional and
explain them in the handoff.
