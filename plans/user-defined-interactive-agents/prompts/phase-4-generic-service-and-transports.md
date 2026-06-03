# Phase 4 prompt — generic service and transports

You are implementing Phase 4 of
`plans/user-defined-interactive-agents/README.md`. Assume Phases 1-3 are done.

## Session bootstrap

Read the normal session docs plus:

- `plans/user-defined-interactive-agents/README.md`
- `docs/reference/interactive-conversation-interface.md`
- `docs/reference/interactive-process-responder-protocol.md`
- interaction spec/reference docs added by earlier phases
- `crates/temper-interaction/src/transport.rs`
- `crates/temper-production/src/product_chat_service.rs`
- `crates/temper-production/src/product_chat_repl.rs`
- `crates/temper-production/src/product_chat_args.rs`

## Goal

Provide a generic deployable interaction service/CLI that loads compiled
user-defined profiles and exposes profile-neutral REPL + HTTP/event APIs.
Product-manager-specific binaries/routes may remain as compatibility aliases, but
new code should use generic names and compiled manifests.

## Tasks

1. Add generic production argument parsing and binary entry point.

   Suggested names are flexible, for example:

   - `temper-interaction repl --spec <path> --profile <id> ...`
   - `temper-interaction serve --spec <path> ...`

   The binary should load an interaction spec, validate/compile it, bind Forge
   identities/tokens and process responders by profile/responder id, then run the
   generic runtime.

2. Define deployment binding config separate from the profile spec for secrets
   and local paths:

   - Forge token env names or role/user bindings;
   - process responder command/args/cwd/env allow-list/timeout;
   - bind address and service token;
   - repository selector.

   User profile specs define behavior; deployment bindings provide credentials
   and executable paths.

3. Generalize the REPL.

   - Render help/available commands from the command manifest.
   - Interpret aliases such as `/file` through generic command actions.
   - Display proposals using proposal manifests, not product-manager DTOs.
   - Keep local commands out of the responder transcript unless the command
     manifest explicitly says to record them.

4. Generalize HTTP/event service routes.

   Preferred profile-neutral routes remain:

   ```text
   POST /conversations
   GET  /conversations/{id}
   POST /conversations/{id}/turns
   GET  /conversations/{id}/proposals
   GET  /conversations/{id}/events
   POST /conversations/{id}/proposals/{proposal_id}/accept
   ```

   Add SSE if it is now small; otherwise keep the event snapshot contract and
   document the streaming follow-up. Do not introduce product-manager route names
   in the generic service.

5. Keep compatibility aliases:

   - `temper-product-manager-chat` may call the generic binary/runtime with the
     product-manager fixture spec;
   - `/sessions` and `/drafts/{slug}/file` may remain as aliases during this
     phase if tests and dogfood still use them.

6. Add tests for:

   - generic CLI/env parsing without product-manager env names;
   - generic REPL command mapping;
   - generic HTTP route behavior across at least two profile ids;
   - compatibility alias still working;
   - non-loopback bind safety/auth.

7. Update docs and plan status.

## Constraints

- Secrets travel by env/config files, never argv/logs.
- Frontends call Temper's interaction service, not responder processes directly.
- Do not depend on Smith or any provider SDK.
- Keep loopback safe defaults.

## Validation

Run and record:

```sh
cargo fmt --all
cargo test -p temper-interaction --all-targets
cargo test -p temper-production --all-targets
cargo dev-clippy
cargo dev-check
```
