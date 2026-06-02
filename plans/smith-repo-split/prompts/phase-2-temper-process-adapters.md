# Phase 2 prompt — Temper process adapters and conformance tests

You are implementing Phase 2 of `plans/smith-repo-split/README.md`. Assume Phase
1 is done and the coverage ledger exists.

## Read first

- `plans/smith-repo-split/README.md`
- `plans/smith-repo-split/coverage-ledger.md`
- `docs/reference/interactive-conversation-interface.md`
- `docs/reference/workflow-layer.md`
- `crates/temper-interaction/`
- `crates/temper-runner/src/agent.rs`
- `crates/temper-production/src/worker.rs`
- `crates/temper-agents/src/role.rs`

## Goal

Make Temper capable of using concrete agents out of process without depending on
Smith or any pi-SDK code.

## Tasks

1. Ensure the interactive responder process adapter from the completed
   interaction plan is covered by hermetic tests using fake commands/scripts.
   Tighten it if the coverage ledger found gaps.

2. Add or finish a workflow role decision process adapter.

   The adapter should:

   - build the versioned workflow decision request from a `RoleManifest`, work
     item context, authorized actions, and bound external-tool metadata;
   - invoke a configured command with deterministic stdin/stdout behavior;
   - parse one decision reply;
   - validate `protocol_version`, `action`, and duplicate/unknown fields as the
     chosen contract requires;
   - treat unknown/unauthorized actions as no-action or an agent error according
     to the existing generic role-agent semantics;
   - execute valid actions only through `RoleTools`.

3. Keep process adapter tests hermetic.

   Use tiny fake responder binaries/scripts or test helpers that return valid,
   malformed, slow, failing, and unauthorized replies. Do not call real LLMs.

4. Update production configuration docs/args enough that operators can select a
   process responder later, but do not require Smith to exist yet.

5. Update the coverage ledger: identify which Temper tests now replace old
   in-process unit coverage and which tests still wait for Smith.

6. Update Phase 2 status in the plan README.

## Constraints

- Temper must not depend on Smith.
- Do not add pi SDK or provider auth dependencies to `temper-interaction`,
  `temper-runner`, `temper-workflow`, or `temper-production`.
- Do not expose Forge tokens/provider secrets to process responders.
- Preserve existing product-chat and worker defaults unless a Smith command is
  explicitly configured.

## Validation

Run:

```sh
cargo fmt --all
cargo test -p temper-interaction
cargo test -p temper-production product_chat
cargo test -p temper-runner
cargo dev-clippy
cargo dev-check
```

Add any focused tests introduced for the process adapters. If ignored e2e tests
are affected by worker configuration, run the relevant fake-agent Forgejo e2e as
well or leave the phase blocked.
