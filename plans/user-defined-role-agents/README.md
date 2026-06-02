# User-defined role agents and prompt compilation — implementation plan

This plan implements the direction agreed in the dogfood investigation:
workflow roles are user-defined, production code must not ship hard-coded role
prompts, and the workflow compiler is responsible for producing the minimal
mechanical prompt/tool surface for each role.

Hand this plan to agents one phase at a time, in order. Each phase should land
with the status updated here and with the full closure suite recorded in the
handoff.

## Target architecture

- **No production hard-coded workflow-role prompts.** If a test needs fixed role
  prose, it lives in test/demo fixtures, not in `harness-agents/src/prompts/`.
- **Generated mechanics.** `harness-workflow` compiles each user-defined role into
  a prompt that contains only workflow mechanics: role id, queues, current work
  item context contract, authorized workflow actions, output format, and the rule
  that authority comes only from the manifest.
- **User extensions.** Users may add role guidance in the workflow spec, e.g. how
  an engineer should implement code, what standards to follow, and what declared
  tools it should use. This prose guides behavior but grants no authority.
- **Declared tools only.** Workflow-transition tools are derived from the
  workflow. Non-workflow tools, such as a coding workspace, must be declared by
  the user and bound by the runner before the LLM may use them.
- **Generic LLM role adapter.** `harness-agents` should provide a manifest-driven
  role agent, not one Rust module/prompt/decision enum per reference role.
- **Dogfood safety.** Engineer automation remains disabled until a real coding
  workspace path can produce product-code diffs; synthetic/bookkeeping PR prep is
  not a substitute.

## Non-goals

- Do not add broad Forge mutation tools for LLMs.
- Do not make the workflow compiler responsible for executing external tools.
- Do not re-enable dogfood engineer automation before the coding workspace phase.
- Do not preserve reference-delivery hard-coded role behavior in production code
  just to keep old tests convenient.

## Full closure suite for every phase

Before a phase is marked done, run every available validation path, including
ignored e2e and real-agent tests. If a gate is unavailable on the machine, the
phase is not complete under this plan; either provision the missing prerequisite
or leave the phase explicitly blocked.

Minimum commands to record in the handoff:

```sh
cargo fmt --all
cargo test --workspace --all-targets
cargo dev-clippy
cargo dev-check

# Process/e2e tests that are ignored by default.
cargo test -p harness-testing --test multiprocess -- --ignored --test-threads=1
cargo test -p harness-testing --test multi_repo_multiprocess -- --ignored --test-threads=1

# Real Forgejo + real host-mode CI, fake agents.
HARNESS_FORGEJO_E2E=1 \
  cargo test -p harness-testing -- --ignored --test-threads=1

# Real Forgejo + real host-mode CI + real LLM agents.
HARNESS_FORGEJO_E2E=1 HARNESS_FORGEJO_AGENTS=1 \
  cargo test -p harness-testing --test forgejo_multiprocess -- --ignored --test-threads=1
HARNESS_FORGEJO_E2E=1 HARNESS_FORGEJO_AGENTS=1 \
  cargo test -p harness-agents --test forgejo_engineer_e2e -- --ignored --test-threads=1
```

Also run any live provider gates that are configured locally, for example
`HARNESS_CHATGPT_OAUTH=1`, `HARNESS_ANTHROPIC_OAUTH=1`, and
`HARNESS_FORGEJO_LIVE=1`/`HARNESS_FORGEJO_LIVE_MUTATE=1` tests. If Python or
shell tooling is touched, include its focused tests too.

## Phases

Status legend: ☐ pending · ☑ done · ⚠ blocked

1. ☑ **Phase 1 — Workflow prompt-extension contract.**
   `prompts/phase-1-workflow-prompt-extension-contract.md`

   Add an explicit user-extension contract to the workflow spec, keeping
   backwards compatibility for existing `charter` text if needed. The compiler
   should render deterministic, minimal mechanical prompt sections plus clearly
   separated user guidance/tool guidance sections. Update validation so unknown
   prompt/tool fields are rejected and update `docs/reference/workflow-layer.md`
   and related explanation docs. Add compiler tests proving no role-specific
   prose is generated and user extensions render deterministically.

2. ☐ **Phase 2 — Manifest-driven LLM role agent.**
   `prompts/phase-2-manifest-driven-llm-role-agent.md`

   Introduce a generic role agent in `harness-agents` that takes a compiled
   `RoleManifest`, uses `role.prompt.render()` as the system prompt, asks for a
   generic decision such as `{ "action": "<manifest tool>", "reason": "..." }`,
   and executes only authorized manifest transitions through `RoleTools`. Add a
   mockable decision seam so unit tests do not call an LLM. Keep stale-work
   handling and provider-error behavior equivalent to the current adapters.

3. ☐ **Phase 3 — Production registry uses user-defined roles.**
   `prompts/phase-3-production-registry-user-defined-roles.md`

   Replace `real_registry_with` production wiring with a builder that registers
   one generic LLM agent per compiled role. Remove hard-coded workflow role ids,
   role prompts, role-specific decision enums, and role-specific adapters from
   the production path. Any reference-delivery prompt text needed by tests moves
   into workflow/test fixtures under `harness-testing` or a plan-specific fixture
   directory. Update live OAuth smoke tests to exercise the generic agent with a
   fixture role instead of importing `ARCHITECT_SYSTEM_PROMPT`.

4. ☐ **Phase 4 — User-declared non-workflow tool manifests.**
   `prompts/phase-4-user-declared-tool-manifests.md`

   Extend role manifests with user-declared non-workflow tool metadata: id,
   description, constraints, and optional prompt guidance. Render those tools in
   the prompt as available only when the runner binds a matching provider.
   Implement runner-side validation that fails fast if a role's prompt declares a
   required external tool with no binding, and proves undeclared tools are never
   registered with the LLM SDK. This phase may expose no executable external
   tools yet; it establishes the authority and prompt contract.

5. ☐ **Phase 5 — Coding workspace seam for engineering roles.**
   `prompts/phase-5-coding-workspace-seam.md`

   Add the first real external tool provider: a coding workspace/coder seam that
   can check out a repo, edit code, commit a meaningful branch, and report the
   resulting head to the workflow agent. Keep Forge/workflow mutation behind
   `RoleTools`; the coding seam may touch git/workspace state only through its
   narrow interface. Update Forgejo PR prep so production dogfood refuses to open
   a PR unless the workspace produced a real non-bookkeeping diff. Add tests that
   reject synthetic-only diffs and accept a fixture product-code change.

6. ☐ **Phase 6 — Dogfood/reference migration and re-enable criteria.**
   `prompts/phase-6-dogfood-reference-migration.md`

   Move any remaining reference-delivery role prose into workflow config or test
   fixtures. Update `examples/dogfood` so engineer automation can be enabled only
   when the coding workspace binding and role guidance are present. Add a
   dogfood preflight that explains why an eligible `code + ready` issue is idle
   when the engineer role lacks a safe coding binding. Do not flip the default to
   enabled until a real dogfood issue is implemented by the workspace path and
   the PR diff guard passes.

7. ☐ **Phase 7 — Remove legacy surfaces and document the model.**
   `prompts/phase-7-remove-legacy-surfaces.md`

   Delete or make test-only any remaining hard-coded workflow-role prompt files
   and role-specific production adapters. Update `AGENTS.md`, `harness-agents`
   docs, workflow reference docs, and dogfood docs so future agents know:
   generated prompts handle mechanics, user config handles role behavior, and
   external tools require explicit declarations plus runner bindings. Add a grep
   style regression test or CI check that production `harness-agents` has no
   checked-in workflow-role prompt files.

## Acceptance criteria

- A workflow with arbitrary role ids can run real LLM role workers without adding
  Rust prompt/adaptor code for those ids.
- Generated prompts contain mechanical workflow instructions plus user-provided
  extensions, not built-in role behavior.
- User-declared external tools are visible to the prompt only when authorized and
  bound by the runner.
- Reference/dogfood prompts used for testing are fixtures, not production code.
- Dogfood cannot create bookkeeping-only PRs; engineer automation has a real
  coding workspace prerequisite.
- Every phase closes only after the full closure suite above has passed.

## Starting points

- `crates/harness-workflow/src/spec.rs`
- `crates/harness-workflow/src/compile.rs`
- `crates/harness-workflow/tests/compilation.rs`
- `crates/harness-agents/src/registry.rs`
- `crates/harness-agents/src/decision.rs`
- `crates/harness-agents/src/prompts/`
- `crates/harness-runner/src/agent.rs`
- `crates/harness-production/src/worker.rs`
- `crates/harness-production/src/forgejo_prep.rs`
- `crates/harness-production/src/pr_diff_guard.rs`
- `examples/dogfood/config/dogfood.env`
- `docs/reference/agent-lessons/0020-dogfood-prs-must-not-be-bookkeeping-only.md`
