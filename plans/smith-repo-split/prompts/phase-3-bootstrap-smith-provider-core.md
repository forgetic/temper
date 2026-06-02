# Phase 3 prompt — bootstrap Smith and move provider core

You are implementing Phase 3 of `plans/smith-repo-split/README.md`. Assume
Temper's process contracts/adapters are ready and tested. This is the first phase
that creates or edits the new Smith repository.

## Read first

- `plans/smith-repo-split/README.md`
- `plans/smith-repo-split/coverage-ledger.md`
- `docs/reference/llm-agents.md`
- `docs/how-to/use-chatgpt-oauth-auth.md`
- `crates/temper-agents/src/provider.rs`
- `crates/temper-agents/src/provider/`
- `crates/temper-agents/src/decision.rs`
- `crates/temper-agents/tests/chatgpt_oauth_live.rs`
- `crates/temper-agents/tests/anthropic_oauth_live.rs`

## Goal

Create `~/src/rust/smith` as a separate Rust workspace and move/copy the
pi-SDK/provider/auth/decision core there with equivalent tests before deleting
anything from Temper.

## Tasks

1. Create the Smith repository at:

   ```text
   ~/src/rust/smith
   ```

   Suggested initial shape:

   ```text
   smith/
     Cargo.toml
     README.md
     crates/smith-temper-agent/      # library: provider/auth/decision code
     crates/smith-temper-agent-cli/  # binaries for process protocols, if useful
   ```

   The exact crate names may differ, but keep them clear and Temper-specific
   enough that future Smith features can coexist.

2. Add a local path dependency from Smith to the current Temper checkout only for
   protocol/domain crates needed by tests and binaries. Temper must not depend on
   Smith.

3. Move or copy provider/auth/decision code from `temper-agents` into Smith.

   Preserve important behavior:

   - tolerant ChatGPT OAuth auth-file parsing/write-back;
   - Anthropic OAuth Claude Code system identity handling;
   - DeepSeek/OpenAI-compatible API-key route;
   - redaction of secrets in errors/logs;
   - transitive dependency pins/workarounds needed for `pi_agent_rust`.

4. Move or duplicate provider unit tests and live-provider tests into Smith.
   Keep the Temper originals until Smith tests pass and the coverage ledger says
   they can be removed in a later phase.

5. Add Smith README instructions for auth, live test env vars, and the local
   Temper path dependency.

6. Update the coverage ledger and Phase 3 status.

## Constraints

- Do not remove `temper-agents` code from Temper yet unless the coverage ledger
  explicitly marks an item as safely replaced.
- Do not copy credentials, auth files, or local secrets into Smith.
- Do not make Smith tests require live provider credentials by default; keep live
  tests ignored/env-gated.
- Keep Smith source files under the same 600-line convention where practical.

## Validation

In Temper:

```sh
cargo fmt --all
cargo dev-check
```

In Smith:

```sh
cargo fmt --all
cargo test --workspace --all-targets
```

When credentials are available, also run the moved live-provider tests in Smith.
If they cannot run locally, document the exact env vars/commands and leave them
ignored rather than deleting them.
