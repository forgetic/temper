# Phase 4 — User-declared non-workflow tool manifests

You are implementing Phase 4 of `plans/user-defined-role-agents/README.md`.
Phases 1–3 should be complete: production real role workers are manifest-driven
and role prompts are generated from workflow specs.

The goal is to let users declare non-workflow tools for a role without granting
implicit authority or registering undeclared SDK tools.

## Session bootstrap

1. Confirm you are in `/home/free/src/rust/temper`.
2. Read `AGENTS.md`, `plans/user-defined-role-agents/README.md`, and Phases 1–3
   diffs.
3. Read:
   - `crates/temper-workflow/src/spec.rs`
   - `crates/temper-workflow/src/compile.rs`
   - the generic role-agent module in `crates/temper-agents`
   - `crates/temper-runner/src/agent.rs`
   - `crates/temper-runner/src/config.rs`
   - `crates/temper-production/src/worker.rs`
4. Re-read the role prompt contract in `docs/reference/workflow-layer.md` and
   `docs/explanation/agentic-workflows.md`.

## Task

Add user-declared external tool metadata to role manifests and runner binding
validation.

1. **Workflow schema.** Add a role-level external tool declaration shape, for
   example:

   ```json
   {
     "id": "engineer",
     "external_tools": [
       {
         "id": "coding_workspace",
         "description": "Edit and commit repository code.",
         "required": true,
         "guidance": "Use it before opening implementation PRs."
       }
     ]
   }
   ```

   Choose final field names deliberately and document them. Use typed ids and
   `#[serde(deny_unknown_fields)]`.

2. **Compilation.** Extend `RoleManifest` with external tool manifests. Render a
   prompt section that distinguishes:
   - authorized workflow actions derived from transitions; and
   - user-declared external tools, which are available only when the runner binds
     matching providers.

   Do not imply unbound tools are executable.

3. **Runner binding contract.** Add a runner-side configuration/binding surface
   for external tools. It may be metadata-only in this phase, but it must support
   validation: required declared tools must have bindings before a real LLM role
   worker starts. Optional unbound tools should be omitted or marked unavailable
   in the runtime prompt/context.

4. **LLM adapter.** Update the generic role agent so the prompt/context it sends
   to the model includes only declared-and-bound external tools. Do not register
   any SDK tools in this phase unless there is a matching binding. Prove that
   undeclared tools are never registered.

5. **Tests.** Add hermetic tests proving:
   - external tool declarations validate and compile deterministically;
   - unknown external-tool fields are rejected;
   - required unbound tools fail preflight with a clear error;
   - optional unbound tools do not appear as available;
   - a bound declared tool appears in the manifest/prompt/context;
   - an undeclared tool binding does not silently grant authority.

6. **Docs.** Update reference docs to make the authority model explicit: prompt
   guidance can mention tools, but a tool is usable only when declared by the
   workflow and bound by the runner.

7. **Plan status.** Mark Phase 4 complete only after the full closure suite
   passes.

## Constraints

- Do not implement the coding workspace itself in this phase; Phase 5 does that.
- Do not add generic Forge mutation tools for the LLM.
- Keep `temper-workflow` provider-agnostic and execution-free.
- Keep `temper-agents` the only crate depending on the LLM SDK.

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
`docs/how-to/end-a-development-session.md` and update the phase status.
