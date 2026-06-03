# Phase 2 prompt — compiled profile manifests

You are implementing Phase 2 of
`plans/user-defined-interactive-agents/README.md`. Assume Phase 1 is done.

## Session bootstrap

Read the normal session docs plus:

- `plans/user-defined-interactive-agents/README.md`
- Phase 1 changes and tests
- `docs/reference/interaction-profile-spec.md` if it exists
- `docs/reference/interactive-conversation-interface.md`
- `crates/temper-interaction/src/{spec,validated,transcript,session,proposal,transport}.rs`
- `crates/temper-production/src/product_chat.rs`
- `crates/temper-production/src/product_chat_service.rs`

## Goal

Compile validated interaction profiles into deterministic runtime manifests and
begin feeding those manifests into the existing session/service path. Concrete
product-manager constants should start disappearing from production logic because
they are supplied by the compiled fixture profile.

## Tasks

1. Add a compile phase in `temper-interaction`:

   ```text
   ValidatedInteractionSpec -> CompiledInteractionSpec
   ```

   Suggested manifest pieces:

   - `ProfileManifest`: profile id, participants, recent-turn policy;
   - `TranscriptManifest`: target, labels, title prefix, marker namespace;
   - `ResponderManifest`: responder id, protocol/version, required flag;
   - `ProposalManifest`: proposal kind ids and payload validators;
   - `CommandManifest`: transport-facing command ids/aliases/action mapping;
   - `AcceptanceManifest`: accepted-action id, proposal kind, idempotency key
     template, declared effects.

2. Make compilation deterministic and infallible once validation succeeded,
   mirroring the workflow compiler pattern.

3. Refactor `ForgeTranscriptConfig`, `ForgeSessionConfig`, and related session
   open options so they can be constructed from a compiled profile manifest.
   Keep existing constructors if useful for tests, but the production path should
   be able to use manifests.

4. Refactor product-chat compatibility code enough that these values come from
   the product-manager fixture manifest rather than constants in runtime logic:

   - profile id;
   - transcript label;
   - workflow intake label / accepted issue labels;
   - title prefix;
   - marker namespace;
   - participant display names;
   - recent-turn limit where applicable.

   It is okay for compatibility type names like `ProductChatSession` to remain
   in this phase, but they should wrap a generic manifest-driven session.

5. Add tests for:

   - deterministic compilation snapshots or structured assertions;
   - constructing session/transcript configs from arbitrary profile manifests;
   - product-manager compatibility using the fixture manifest;
   - no literal `product-manager` branch in compiler/session logic.

6. Update docs and the plan status.

## Constraints

- Do not add provider SDK dependencies.
- Do not broaden responder authority.
- Do not remove product-manager operator entry points yet.
- Prefer manifest data over special-case branches.

## Validation

Run and record:

```sh
cargo fmt --all
cargo test -p temper-interaction --all-targets
cargo test -p temper-production product_chat
cargo dev-clippy
cargo dev-check
```
