# Phase 1 prompt — contracts and coverage ledger

You are implementing Phase 1 of `plans/smith-repo-split/README.md`. Assume
`plans/interactive-agent-interfaces/` is complete. Do not create the Smith repo
yet except for exploratory notes; this phase freezes the contracts and test map.

## Read first

- `README.md`
- `AGENTS.md`
- `docs/reference/development-conventions.md`
- `docs/reference/llm-agents.md`
- `docs/reference/interactive-conversation-interface.md`
- `docs/explanation/interactive-agent-interfaces.md`
- `plans/interactive-agent-interfaces/README.md`
- `plans/smith-repo-split/README.md`
- `crates/temper-agents/src/decision.rs`
- `crates/temper-agents/src/role.rs`
- `crates/temper-agents/src/product_manager.rs`
- `crates/temper-agents/tests/`

## Goal

Define the exact process-boundary contracts Smith will implement and write a
coverage ledger so the split does not quietly drop tests, especially real-world
and real-agent e2e coverage.

## Tasks

1. Inventory current concrete-agent code and tests.

   Include at least:

   - provider/auth/OAuth tests;
   - product-manager tests;
   - generic role-agent/unit tests;
   - live provider tests;
   - Forgejo real-agent/e2e tests;
   - reference-delivery or product-chat real-world rehearsals.

2. Add a coverage ledger under this plan, suggested path:

   ```text
   plans/smith-repo-split/coverage-ledger.md
   ```

   For each test or suite, record:

   - current path/command;
   - intended post-split home: Temper, Smith, or both;
   - whether it is hermetic, live-provider, Forgejo e2e, or real-world;
   - whether it can be moved immediately or must stay until a later phase;
   - equivalent command expected after the split.

3. Define or document the process protocol structs/fixtures in Temper.

   Prefer contract types in existing provider-neutral crates. If code already
   exists from the completed interaction plan, update docs/fixtures rather than
   inventing parallel types.

   Required protocol families:

   - interactive responder request/reply;
   - workflow role decision request/reply.

4. Add JSON fixture examples for both protocols. They should be usable by Smith
   tests later and by Temper process-adapter conformance tests.

5. Update `plans/smith-repo-split/README.md` Phase 1 status and point to the
   coverage ledger.

## Constraints

- Do not move pi-SDK code yet.
- Do not remove or delete tests in this phase.
- Do not make Temper depend on Smith.
- Do not pass Forge credentials or provider secrets in protocol examples.
- Keep docs concise; split protocol examples into fixtures if they get long.

## Validation

Run at least:

```sh
cargo fmt --all
cargo test -p temper-interaction
cargo dev-clippy
cargo dev-check
```

If this phase is docs/fixtures only and a narrower validation is justified,
state that explicitly in the handoff. Do not mark the phase done without the
coverage ledger.
