# Phase 1 — Workflow prompt-extension contract

You are implementing Phase 1 of `plans/user-defined-role-agents/README.md`.
The goal is to make the workflow compiler the source of production role prompts:
it renders only workflow mechanics plus explicit user-authored prompt extensions.

## Session bootstrap

1. Confirm you are in `/home/free/src/rust/temper`.
2. Read `AGENTS.md`, `docs/README.md`, and the active lessons relevant to this
   task, especially:
   - `docs/reference/agent-lessons/0020-dogfood-prs-must-not-be-bookkeeping-only.md`
   - `docs/reference/agent-lessons/0021-user-defined-roles-own-prompt-behavior.md`
3. Read `plans/user-defined-role-agents/README.md`.
4. Read the workflow compilation and validation surfaces:
   - `crates/temper-workflow/src/spec.rs`
   - `crates/temper-workflow/src/validated.rs`
   - `crates/temper-workflow/src/validate.rs`
   - `crates/temper-workflow/src/compile.rs`
   - `crates/temper-workflow/tests/compilation.rs`
5. Read the docs you will update:
   - `docs/reference/workflow-layer.md`
   - `docs/explanation/agentic-workflows.md`
   - `docs/adr/0007-workflow-layer-and-agent-compilation.md`

## Task

Add an explicit user prompt-extension contract to workflow roles and render it in
compiled role prompts.

1. **Schema.** Add a role-level prompt extension field to the raw workflow spec.
   Use a small structured shape with `#[serde(deny_unknown_fields)]`, for
   example:

   ```json
   {
     "id": "engineer",
     "prompt": {
       "guidance": "How this user wants the role to do its work.",
       "tool_guidance": "How this role should use declared tools."
     }
   }
   ```

   Keep existing `charter` input backwards-compatible. Treat it as user guidance,
   not generated mechanics. Do not put reference-delivery role behavior into the
   compiler.

2. **Validated model.** Carry the prompt extension through validation into the
   validated role model with typed, documented fields. Validation should reject
   unknown prompt fields through serde and should preserve deterministic order.

3. **Prompt rendering.** Update `build_prompt` in `compile.rs` so generated role
   prompts contain minimal mechanics only, with user text clearly separated. The
   prompt should include sections for:
   - role/workflow identity and concurrency;
   - work-item context contract;
   - subscribed queues;
   - authorized workflow actions/tools;
   - user guidance, including legacy `charter` if supplied;
   - user tool guidance, if supplied.

   The mechanical sections must not contain hard-coded role-specific judgment
   such as “engineers implement code” or “reviewers approve PRs”. Those belong in
   user guidance.

4. **Tests.** Update `crates/temper-workflow/tests/compilation.rs` and add any
   smaller validation tests needed. Prove:
   - prompt section order is deterministic;
   - a role with no user prompt extension still renders valid mechanical
     sections;
   - user guidance and user tool guidance render in their own sections;
   - unknown prompt-extension fields are rejected;
   - a synthetic role id such as `banana` gets no role-specific generated prose.

5. **Docs.** Update workflow reference/explanation docs to state the contract:
   generated prompts handle workflow mechanics; user config owns role behavior;
   prompt prose does not grant authority beyond compiled tools.

6. **Plan status.** Mark Phase 1 complete in
   `plans/user-defined-role-agents/README.md` only after the full validation
   suite below passes.

## Constraints

- Keep `temper-workflow` LLM- and provider-agnostic.
- Do not touch `temper-agents/src/prompts/` in this phase except to inspect it
  for contrast.
- Do not change production worker wiring yet.
- Keep docs focused and Rust files under the repository line-budget guidance.

## Done

Run and record the full closure suite from the plan README, including at least:

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

Also run configured live provider gates (`TEMPER_CHATGPT_OAUTH=1`,
`TEMPER_ANTHROPIC_OAUTH=1`, `TEMPER_FORGEJO_LIVE=1`) when available. Follow
`docs/how-to/end-a-development-session.md` and include the validation log in the
handoff.
