# Phase 3 — Production registry uses user-defined roles

You are implementing Phase 3 of `plans/user-defined-role-agents/README.md`.
Phases 1–2 should be complete: role prompts are compiled from workflow specs and
`temper-agents` has a generic manifest-driven role agent.

The goal is to remove hard-coded workflow role prompts/ids from the production
worker path. Reference-delivery behavior needed for tests may move to test-level
fixtures, but production wiring must be user-role driven.

## Session bootstrap

1. Confirm you are in `/home/free/src/rust/temper`.
2. Read `AGENTS.md`, `plans/user-defined-role-agents/README.md`, and the Phase 1
   and Phase 2 diffs.
3. Read:
   - `crates/temper-agents/src/registry.rs`
   - the new generic role-agent module from Phase 2
   - `crates/temper-production/src/worker.rs`
   - `crates/temper-production/src/worker_args.rs`
   - `crates/temper-testing/src/worker_bin/`
   - `crates/temper-testing/tests/forgejo_multiprocess.rs`
4. Read the current reference-delivery workflow fixture and any prompt-extension
   data added in Phase 1:
   `crates/temper-workflow/fixtures/reference-delivery.json`.

## Task

Switch real production role-worker registration to compiled workflow manifests.

1. **Registry API.** Replace or supplement `real_registry_with` with a builder
   that takes a compiled workflow (or `ValidatedWorkflow` and compiles it once)
   and registers one generic LLM role agent per compiled role. The role ids must
   come from the workflow, not from a hard-coded list.

2. **Production worker wiring.** Update `temper-production` so `--agents real`
   builds the registry from the workflow manifests. Remove production dependence
   on `ENGINEER_SYSTEM_PROMPT`, `ARCHITECT_SYSTEM_PROMPT`, etc. The product
   manager conversational prompt is not a workflow-role prompt; do not mix that
   path into this change.

3. **Reference fixtures.** Move any reference-delivery role guidance needed by
   real-agent tests into workflow config or test/demo fixture data. Do not keep
   reference workflow judgment in `temper-agents/src/prompts/` for production
   role workers.

4. **Compatibility.** Keep deterministic fake agents and existing fake-agent e2e
   behavior intact. If a reference real-agent e2e needs behavior that cannot be
   expressed as a manifest transition decision, add the smallest explicit
   test-level fixture/binding needed and document why. Do not reintroduce
   production role-specific prompts.

5. **Live tests.** Update live OAuth smoke tests so they exercise a compiled
   fixture role prompt through the generic decision path instead of importing a
   checked-in role prompt constant such as `ARCHITECT_SYSTEM_PROMPT`.

6. **Tests.** Add unit/integration tests proving:
   - arbitrary role ids from a synthetic workflow are registered;
   - roles absent from the workflow are not registered;
   - production registry construction does not reference hard-coded role prompt
     constants;
   - the existing Forgejo real-agent e2e still converges.

7. **Plan status.** Mark Phase 3 complete only after the full closure suite
   passes.

## Constraints

- No production hard-coded workflow-role prompts.
- Do not delete legacy prompt files/modules yet if doing so would make the
  migration too large; Phase 7 is the cleanup phase. But they must be unused by
  production role workers after this phase.
- Keep dogfood engineer automation disabled unless the coding workspace safety
  from later phases exists.
- Secrets remain env/file only; never argv/logs.

## Done

Run and record the full closure suite from the plan README:

```sh
cargo fmt --all
cargo test --workspace --all-targets
cargo dev-clippy
cargo dev-check
cargo test -p temper-testing --test multiprocess -- --ignored --test-threads=1
cargo test -p temper-testing --test multi_repo_multiprocess -- --ignored --test-threads=1
TEMPER_FORGEJO_E2E=1 cargo test -p temper-testing -- --ignored --test-threads=1
TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 \
  cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --test-threads=1
TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 \
  cargo test -p temper-agents --test forgejo_engineer_e2e -- --ignored --test-threads=1
```

Run configured live provider gates when available. Follow
`docs/how-to/end-a-development-session.md`, update docs/handoff, and mark Phase 3
in the README.
