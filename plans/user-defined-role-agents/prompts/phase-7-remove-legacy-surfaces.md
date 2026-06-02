# Phase 7 — Remove legacy surfaces and document the model

You are implementing Phase 7 of `plans/user-defined-role-agents/README.md`.
Phases 1–6 should be complete: production role agents are user-defined and
manifest-driven, external tools are declared/bound, dogfood has safe re-enable
criteria, and reference behavior has moved to user config or test fixtures.

The goal is cleanup: delete or make test-only the old hard-coded workflow-role
prompt/adaptor surfaces and document the final model.

## Session bootstrap

1. Confirm you are in `/home/free/src/rust/temper`.
2. Read `AGENTS.md`, `plans/user-defined-role-agents/README.md`, and Phases 1–6
   diffs.
3. Read:
   - `crates/temper-agents/src/lib.rs`
   - `crates/temper-agents/src/prompts.rs`
   - `crates/temper-agents/src/prompts/`
   - `crates/temper-agents/src/registry.rs`
   - `docs/reference/workflow-layer.md`
   - `docs/explanation/agentic-workflows.md`
   - `examples/dogfood/README.md`
4. Re-read `docs/reference/agent-lessons/0021-user-defined-roles-own-prompt-behavior.md`.

## Task

Remove legacy hard-coded workflow-role prompt/adaptor surfaces from production
code and add regressions/docs so they do not come back.

1. **Delete or quarantine legacy prompts.** Remove production exports for
   workflow-role prompt constants such as engineer/architect/reviewer/owner/human.
   If a fixed prompt is still needed for a test, move it under a test fixture or
   plan/demo fixture path outside production `temper-agents` role-worker code.
   The non-workflow product-manager conversational prompt is outside this rule;
   keep it only if its docs clearly identify it as non-workflow.

2. **Remove legacy adapters.** Delete or make test-only old role-specific LLM
   adapters and decision enums once no production/test path depends on them. The
   production registry should contain only generic manifest-driven role agents.

3. **Regression check.** Add a grep-style test or CI/check test proving production
   `temper-agents` does not ship checked-in workflow-role prompt files or import
   role-specific prompt constants. Keep the check precise enough not to flag the
   product-manager conversational prompt or test fixtures.

4. **Docs.** Update:
   - `crates/temper-agents/src/lib.rs` crate docs;
   - `docs/reference/workflow-layer.md`;
   - `docs/explanation/agentic-workflows.md`;
   - `examples/dogfood/README.md`;
   - `AGENTS.md` if future agents need the rule during bootstrap.

   State plainly: generated prompts carry mechanics, user workflow config carries
   role behavior, and external tools require explicit declarations plus runner
   bindings.

5. **Search.** Use `rg` to verify there are no production references to legacy
   workflow-role prompt constants/files. Include the search command and result in
   the handoff.

6. **Plan status.** Mark Phase 7 complete only after the full closure suite
   passes.

## Constraints

- Do not delete deterministic fake agents in `temper-testing`; they are test
  behavior fixtures, not production LLM prompts.
- Do not remove the product-manager conversational path unless it has become
  unused; it is not a workflow-role adapter.
- Do not weaken dogfood PR-diff safety while cleaning up.
- Keep docs focused and source files under line-budget guidance.

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
`docs/how-to/end-a-development-session.md`, include the `rg` audit result, and
mark the whole plan complete if all acceptance criteria are met.
