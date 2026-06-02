# Phase 2 — Manifest-driven LLM role agent

You are implementing Phase 2 of `plans/user-defined-role-agents/README.md`.
Phase 1 should be complete: compiled role prompts now contain workflow mechanics
plus user prompt extensions.

The goal is to add a generic LLM role adapter that consumes a `RoleManifest`
instead of hard-coded role prompts and role-specific Rust decision enums. Do not
switch production workers to it yet; that is Phase 3.

## Session bootstrap

1. Confirm you are in `/home/free/src/rust/temper`.
2. Read `AGENTS.md`, `plans/user-defined-role-agents/README.md`, and Phase 1's
   implementation/diffs.
3. Read:
   - `crates/temper-agents/src/decision.rs`
   - `crates/temper-agents/src/common.rs`
   - `crates/temper-agents/src/registry.rs`
   - `crates/temper-runner/src/agent.rs`
   - `crates/temper-workflow/src/compile.rs`
4. Read the current role adapters only to understand behavior you are replacing:
   `architect.rs`, `engineer.rs`, `reviewer.rs`, `owner.rs`, and `human.rs`.

## Task

Introduce a generic, manifest-driven LLM role agent in `temper-agents`.

1. **Generic decision type.** Add a role decision shape such as:

   ```json
   { "action": "<authorized_action_or_no_action>", "reason": "short rationale" }
   ```

   The action must be matched against the supplied `RoleManifest.tools` before
   any `RoleTools` call happens. Unknown actions must never execute.

2. **Mockable decision seam.** Add a small async decision trait or equivalent
   seam so unit tests can inject decisions without making LLM calls. The provider
   implementation should keep using `run_decision` with `role.prompt.render()`.
   A provider setup error remains a real `AgentError`; transient parse/model
   failures should degrade to no-action as the current adapters do.

3. **Generic agent.** Implement something like `LlmRoleAgent` that owns:
   - the compiled `RoleManifest`;
   - the provider-backed or injected decision engine;
   - no role-specific prompt constants or role-specific decision enums.

   In `service`, build the work-item context through `RoleTools`, ask for one
   decision, validate the action against the manifest, and execute the matching
   transition through `tools.run(item.target, transition)`. Treat stale
   precondition/classification/target-missing failures as no-progress, matching
   the existing adapters.

4. **Prompt/context contract.** Ensure the generated system prompt tells the LLM
   the output format and allowed actions. The user message should include the
   current work-item JSON and may include a compact copy of authorized actions
   from the manifest for redundancy. Do not register SDK bash/file tools.

5. **Tests.** Add hermetic tests with fake decisions proving:
   - an authorized transition is executed;
   - `no_action` makes no mutation;
   - an unknown action is rejected or no-ops without running a transition;
   - stale execution errors return `Ok(false)`;
   - provider/decision parse failures are handled as specified;
   - the system prompt passed to the decision engine is the compiled manifest
     prompt, not a checked-in role prompt file.

6. **Plan status.** Mark Phase 2 complete in the README only after the full
   validation suite passes.

## Constraints

- `temper-forge`, `temper-runner`, and `temper-workflow` remain LLM-agnostic.
- Do not remove or production-wire the legacy role adapters yet; keep existing
  e2e behavior green while introducing the generic path.
- Do not add external non-workflow tools in this phase. That is Phase 4.
- Do not make the generic agent special-case role ids like `engineer`.

## Done

Run and record the full closure suite from the plan README, including:

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
`docs/how-to/end-a-development-session.md` and update the plan status/handoff.
