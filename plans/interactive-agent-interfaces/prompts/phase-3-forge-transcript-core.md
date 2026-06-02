# Phase 3 prompt — extract Forge transcript and issue-proposal core

You are implementing Phase 3 of `plans/interactive-agent-interfaces/README.md`.
Assume Phases 1 and 2 are done. Do not recast product-manager as a profile yet
beyond compatibility adapters needed to keep tests passing.

## Read first

- `README.md`
- `AGENTS.md`
- `docs/reference/development-conventions.md`
- `docs/explanation/interactive-agent-interfaces.md`
- `docs/reference/interactive-conversation-interface.md`
- `docs/reference/forge-interface.md`
- `crates/temper-interaction/src/lib.rs` and sibling modules
- `crates/temper-production/src/product_chat.rs`
- `crates/temper-production/src/product_chat_tests.rs`
- `plans/interactive-agent-interfaces/README.md`

## Goal

Move reusable conversation persistence and explicit proposal acceptance out of
product-manager-specific production code into the generic interaction crate.

## Tasks

1. In `temper-interaction`, add a Forge-backed transcript/session layer.
   Suggested split:

   - `transcript.rs`: transcript open/create/resume, marker rendering/parsing,
     recent-turn loading.
   - `session.rs`: append human turn, call `InteractiveResponder`, append agent
     reply, cache latest proposals.
   - `proposal.rs`: idempotent acceptance helpers for issue-intake proposals.
   - `error.rs`: generic interaction/transcript/proposal errors.

2. Make product-specific values configurable rather than hard-coded:

   - transcript label (`product` for product-manager);
   - workflow intake label (`untriaged` for product-manager issue proposals);
   - transcript title prefix;
   - marker namespace/profile id;
   - human and agent Forge handles/users.

3. Preserve the important safety behavior from `product_chat.rs`:

   - transcript issues are distinct from workflow intake issues;
   - resume refuses issues that do not match the configured transcript-label
     policy;
   - recent turns are reconstructed from Forge comments by author identity;
   - issue proposal filing is explicit and idempotent via hidden marker;
   - filed issue body includes a transcript backlink and requested-by field when
     available.

4. Move or duplicate tests from `temper-production` into `temper-interaction`
   until the generic crate owns the behavior. Keep product-manager tests as
   compatibility coverage for the wrapper.

5. Keep `temper-production/src/product_chat.rs` compiling by delegating to the
   generic crate where possible, but do not spend this phase renaming all
   product-manager types.

6. Update docs and Phase 3 status in the plan README.

## Constraints

- `temper-interaction` may depend on `temper-forge`; it still must not depend on
  `temper-agents`, `temper-runner`, `temper-workflow`, `temper-production`, or
  provider SDK crates.
- Do not add new methods to the `Forge` trait unless you prove a portable gap and
  update `docs/reference/forge-interface.md`.
- Keep files under 600 lines by splitting modules before they grow.

## Validation

Run:

```sh
cargo fmt --all
cargo test -p temper-interaction
cargo test -p temper-production product_chat
cargo dev-clippy
cargo dev-check
```
