# Phase 5 — Coding workspace seam for engineering-style roles

You are implementing Phase 5 of `plans/user-defined-role-agents/README.md`.
Phases 1–4 should be complete: roles are user-defined, real role agents are
manifest-driven, and external tools are declared by the workflow and bound by the
runner.

The goal is to add the first real external tool provider: a coding workspace that
can produce meaningful product-code diffs before an implementation PR is opened.

## Session bootstrap

1. Confirm you are in `/home/free/src/rust/temper`.
2. Read `AGENTS.md`, `plans/user-defined-role-agents/README.md`, and Phases 1–4
   diffs.
3. Read dogfood safety context:
   - `docs/reference/agent-lessons/0020-dogfood-prs-must-not-be-bookkeeping-only.md`
   - `examples/dogfood/README.md`
   - `examples/dogfood/config/dogfood.env`
4. Read current PR prep/diff guard code:
   - `crates/temper-production/src/forgejo_prep.rs`
   - `crates/temper-production/src/pr_diff_guard.rs`
   - `crates/temper-production/src/worker.rs`
   - `crates/temper-testing/src/worker_bin/forgejo_engineer.rs`
5. Read the generic agent/external tool binding code from Phases 2–4.

## Task

Implement a safe coding workspace seam and bind it as a declared external tool.

1. **Trait/interface.** Define a narrow coding workspace interface. It should be
   able to:
   - prepare/check out a repository workspace;
   - receive the work-item context and user guidance;
   - make or delegate code edits;
   - commit a branch/head suitable for a PR;
   - report the produced branch/head and a short summary.

   Keep Forge/workflow state mutation behind `RoleTools`. The coding workspace
   may touch git/workspace state only through its own narrow interface.

2. **LLM/external tool binding.** Bind the workspace only when the workflow role
   declares the matching external tool and runner config provides a binding. The
   LLM prompt/context should make clear that implementation work must happen
   through this declared tool before a PR can be opened.

3. **PR creation flow.** Update the generic role path so implementation PR
   creation uses the workspace-produced branch/head. Production Forgejo PR prep
   must refuse to open a PR if the workspace did not produce a real diff.

4. **Diff safety.** Reuse or extend `pr_diff_guard` so production dogfood rejects
   synthetic/bookkeeping-only diffs such as `.temper-pr-prep` or `.temper-ci`
   changes. Add tests that reject synthetic-only diffs and accept a fixture
   product-code change.

5. **Testing implementation.** Provide a deterministic fixture workspace for
   tests/e2e that produces a small real code/docs diff without hard-coded
   production role prompts. Keep any synthetic helper under test-level code.

6. **Docs.** Document how a user declares and binds the coding workspace tool and
   why dogfood engineer automation remains disabled without it.

7. **Plan status.** Mark Phase 5 complete only after the full closure suite
   passes.

## Constraints

- Do not expose broad bash/file tools directly to the LLM.
- Do not let prompt text grant authority; declaration plus binding is required.
- Do not re-enable dogfood engineer automation by default in this phase unless a
   real dogfood issue has been implemented through this workspace and the diff
   guard passes.
- Keep secrets out of argv/logs.
- Keep crate boundaries clean: workflow stays provider/LLM/workspace agnostic.

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

Run configured live provider gates and any new git/workspace focused tests. Follow
`docs/how-to/end-a-development-session.md` and update the phase status.
