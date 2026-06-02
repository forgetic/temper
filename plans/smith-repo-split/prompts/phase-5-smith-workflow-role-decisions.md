# Phase 5 prompt — Smith workflow-role decision responder

You are implementing Phase 5 of `plans/smith-repo-split/README.md`. Assume Smith
owns provider core and the interactive product-manager responder, and Temper has
a workflow role decision process adapter.

## Read first

- `plans/smith-repo-split/README.md`
- `plans/smith-repo-split/coverage-ledger.md`
- `docs/reference/workflow-layer.md`
- `docs/reference/llm-agents.md`
- `crates/temper-agents/src/role.rs`
- `crates/temper-agents/src/registry.rs`
- `crates/temper-agents/tests/forgejo_engineer_e2e.rs`
- `crates/temper-production/src/worker.rs`
- `crates/temper-testing/`
- Smith provider/product-manager crates

## Goal

Move manifest-driven workflow-role LLM decision behavior into Smith and run
Temper workflow workers through the process decision adapter.

## Tasks

1. Implement a Smith binary/command for the workflow role decision protocol.

   It should read a versioned workflow decision request, call the LLM using
   Smith's provider core, and write `{ protocol_version, action, reason }`.

2. Preserve the current generic role-agent behavior:

   - no checked-in workflow-role prompts in production code;
   - generated manifest mechanics plus user-supplied guidance;
   - unauthorized/unknown action handling equivalent to current behavior;
   - provider failures mapped consistently;
   - no SDK bash/file tools registered.

3. Ensure coding-workspace authority remains in Temper.

   Smith can see declared/bound tool metadata and guidance, but it must only
   choose an authorized action. Temper invokes `coding_workspace` and executes
   `CreatePullRequest` transitions through `RoleTools`.

4. Wire Temper production worker configuration so a role can use the Smith
   process decision command. Keep fake-agent/test paths working.

5. Move or duplicate role-agent tests according to the coverage ledger.

   Temper should keep hermetic process-adapter tests. Smith should own provider
   decision tests and the real-agent Forgejo e2e that proves Smith can drive a
   real workflow through Temper.

6. Run or preserve the real-world/e2e gates.

   At minimum, keep equivalents of the existing real LLM + Forgejo commands
   documented and runnable. Do not remove the old Temper e2e until the Smith
   version exists and has passed where prerequisites are available.

7. Update docs, coverage ledger, and Phase 5 status.

## Constraints

- Temper must not depend on Smith as a Rust crate.
- Smith must not mutate Forge/workflow state directly.
- Do not weaken PR diff guard or coding-workspace safety checks.
- Do not reduce ignored/env-gated real-world coverage; move it or keep it until
  a Smith equivalent is ready.

## Validation

In Temper:

```sh
cargo fmt --all
cargo test -p temper-runner
cargo test -p temper-production product_chat
cargo dev-clippy
cargo dev-check
```

In Smith:

```sh
cargo fmt --all
cargo test --workspace --all-targets
```

When available, run the real Forgejo + real LLM e2e through Smith, for example
with the equivalent of:

```sh
TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 \
  cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --test-threads=1
TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 \
  cargo test -p temper-agents --test forgejo_engineer_e2e -- --ignored --test-threads=1
```

The exact post-split Smith commands should be documented in Smith's README and
in the coverage ledger.
