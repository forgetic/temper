# Phase 4 prompt — Smith interactive product-manager responder

You are implementing Phase 4 of `plans/smith-repo-split/README.md`. Assume Smith
exists and owns provider/auth/decision core, and Temper has a process-capable
interactive responder adapter.

## Read first

- `plans/smith-repo-split/README.md`
- `plans/smith-repo-split/coverage-ledger.md`
- `docs/reference/interactive-conversation-interface.md`
- `docs/reference/product-manager-chat-api.md`
- `crates/temper-agents/src/product_manager.rs`
- `crates/temper-agents/src/prompts/product_manager.md`
- `crates/temper-production/src/product_chat*.rs`
- Smith README and provider crates

## Goal

Move product-manager interactive responder behavior into Smith as a process
responder while keeping Temper's product-chat integration, transcript storage,
proposal acceptance, and local API behavior intact.

## Tasks

1. Implement a Smith binary/command for the interactive responder protocol.

   It should read a versioned serialized `ConversationRequest`, run the
   product-manager profile with Smith's provider core, and write one serialized
   `ConversationReply` containing reply text and inert issue proposals.

2. Move or copy the product-manager prompt/profile mapping from Temper to Smith.
   Keep product-manager as an interactive profile, not a workflow role.

3. Preserve tests:

   - product-manager response parsing/slug/proposal tests move to Smith or gain
     Smith equivalents;
   - Temper keeps product-chat transcript/filing/session tests using fake or
     process responders;
   - no test coverage should disappear without a ledger entry.

4. Add Temper configuration for selecting the Smith product-manager command if
   it is not already present from the completed interaction plan.

5. Run product-chat through the Smith process responder when configured. Keep
   existing operator commands stable:

   ```sh
   temper-product-manager-chat repl ...
   temper-product-manager-chat serve ...
   examples/dogfood/run.sh product-chat
   ```

6. Update docs so external frontends still target Temper's interaction service,
   not the Smith process directly.

7. Update the coverage ledger and Phase 4 status.

## Constraints

- Smith responders must not receive Forge handles, Forge tokens, or workflow
  mutation tools.
- Proposal filing stays in Temper's explicit acceptance path.
- Keep live provider tests env-gated.
- Do not remove Temper's old product-manager code until Smith coverage and
  product-chat integration are green.

## Validation

In Smith:

```sh
cargo fmt --all
cargo test --workspace --all-targets product_manager
```

In Temper:

```sh
cargo fmt --all
cargo test -p temper-interaction
cargo test -p temper-production product_chat
cargo dev-clippy
cargo dev-check
```

If dogfood/product-chat real-world validation is available, run it with the Smith
responder configured. If unavailable, keep the command documented and env-gated.
