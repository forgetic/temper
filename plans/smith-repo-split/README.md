# Smith repo split and process-boundary plan

This plan assumes `plans/interactive-agent-interfaces/` has completed: Temper has
provider-neutral interaction types, transcript/proposal/session ownership,
transport-facing APIs, and a process-capable interactive responder adapter.

The follow-up is to split concrete pi-SDK-backed agent implementations out of
Temper into a new Rust repository named **Smith**, checked out locally at:

```text
~/src/rust/smith
```

Hand the prompt files to implementation agents **one phase at a time, in order**.
Each phase should land green and update this README's status.

## Goal

Temper should remain the workflow and interaction runtime. Smith should own the
concrete agent implementation that currently lives behind `pi_agent_rust`:
provider auth, OAuth quirks, model calls, product-manager profile behavior, and
manifest-driven workflow-role decisions.

The stable integration boundary should be a process protocol. Temper sends a
versioned JSON request to a responder/decision process and receives one
versioned JSON reply. Temper still validates every action/proposal and performs
all Forge/workflow mutation itself.

## Ownership boundary

Temper owns:

- `temper-forge`, `temper-workflow`, `temper-runner`, and `temper-interaction`;
- process adapters and protocol contracts;
- workflow role authority, `RoleTools`, gates, leases, and transition execution;
- interactive transcripts, proposal validation, explicit proposal acceptance,
  transport auth, and Forge mutations;
- fake/conformance process responders for hermetic tests.

Smith owns:

- pi-SDK/provider wiring and auth file handling;
- ChatGPT/Anthropic/DeepSeek model-specific behavior;
- product-manager interactive responder implementation;
- manifest-driven workflow-role decision implementation;
- Smith binaries that implement Temper's process protocols;
- live provider tests and real-agent e2e tests moved from Temper.

Temper must not take a Rust dependency on Smith. Smith may depend on Temper
crates by local path during the split and by git/version later.

## Process protocols

Use provider-neutral JSON. New process contracts carry an explicit
`protocol_version`; the completed interactive responder contract reuses the
existing `ConversationRequest`/`ConversationReply` shape and freezes its v1 wire
examples as fixtures. The exact structs should live in Temper's contract crates
and be re-used by Smith.

Suggested commands/protocols:

1. **Interactive responder**
   - request: `temper_interaction::ConversationRequest`
   - reply: `temper_interaction::ConversationReply`
   - fixtures: `crates/temper-interaction/fixtures/interactive-responder-*.json`
   - authority: reply text plus inert proposals only

2. **Workflow role decision**
   - request: `temper_runner::WorkflowRoleDecisionRequest` with workflow id,
     `RoleManifest`, work item context JSON, authorized actions/tools,
     bound external-tool metadata, and no Forge credentials
   - reply: `temper_runner::WorkflowRoleDecisionReply`, where `action` is one
     manifest tool name or `no_action`
   - fixtures: `crates/temper-runner/fixtures/workflow-role-decision-*.json`
   - authority: Temper validates the action against the manifest and executes
     only through `RoleTools`

Secrets stay in Smith's environment/auth files. Temper should not pass provider
secrets, Forge tokens, broad Forge handles, or workflow mutation tools across the
process boundary.

## Coverage rule

Do not reduce practical coverage during the split. Before deleting or moving a
Temper test, add an equivalent Smith test or a Temper-side process-adapter test
and record the mapping in the phase handoff.

Be especially careful with real-world/e2e coverage. The real LLM + Forgejo paths
may stay ignored/env-gated, but they should continue to exist and be runnable
after the split. Do not build duplicate test infrastructure just for purity;
Smith can use Temper's testing crates/fixtures by local path while the split is
in progress.

## Phases

Status legend: ☐ pending · ⚠ blocked · ☑ done

1. ☑ **Phase 1 — Freeze contracts and write the coverage ledger.**
   `prompts/phase-1-contracts-and-coverage-ledger.md`

   Done: added Temper-owned workflow-role decision protocol types/fixtures in
   `temper-runner`, fixture-backed interactive responder examples in
   `temper-interaction`, reference docs for the workflow decision process
   protocol, and the split coverage ledger at `coverage-ledger.md`. No
   `temper-agents` tests were moved or deleted.

2. ☐ **Phase 2 — Add/finish Temper process adapters and conformance tests.**
   `prompts/phase-2-temper-process-adapters.md`

   Ensure Temper can run interactive responders and workflow role decision
   engines through provider-neutral subprocess adapters, using fake responder
   binaries/scripts in tests.

3. ☐ **Phase 3 — Bootstrap `~/src/rust/smith` and move provider core.**
   `prompts/phase-3-bootstrap-smith-provider-core.md`

   Create the Smith workspace, move/copy pi-SDK provider/auth/decision code, and
   preserve unit/live-provider tests there before removing anything from Temper.

4. ☐ **Phase 4 — Smith interactive product-manager responder.**
   `prompts/phase-4-smith-interactive-product-manager.md`

   Implement the product-manager process responder in Smith and point Temper's
   product-chat profile at it when configured. Keep existing product-chat
   commands and compatibility aliases working.

5. ☐ **Phase 5 — Smith workflow-role decision responder.**
   `prompts/phase-5-smith-workflow-role-decisions.md`

   Implement Smith's manifest-driven workflow-role decision process and run the
   real-agent Forgejo/real-world e2e through Temper's process adapter.

6. ☐ **Phase 6 — Remove Temper pi-SDK coupling and close parity.**
   `prompts/phase-6-remove-temper-pi-sdk-coupling.md`

   Delete or deprecate in-repo concrete pi-SDK agent code only after Smith owns
   equivalent coverage and Temper's process-adapter tests are green.

## Acceptance criteria

- Temper builds and tests without `pi_agent_rust` or concrete provider SDKs.
- Temper can run workflow role agents and interactive responders through process
  adapters.
- Smith provides pi-SDK-backed binaries for the same workflow-role and
  product-manager behavior previously available in Temper.
- Tests moved out of Temper have equivalent Smith coverage or Temper
  process-adapter coverage.
- Real LLM + Forgejo e2e coverage remains available and documented, even if
  ignored/env-gated.
- Product-chat and reference/real-world agent paths still work after the split.

## Validation expectations

Temper-side phases should run the normal fast loop plus focused process-adapter
and product-chat tests:

```sh
cargo fmt --all
cargo test -p temper-interaction
cargo test -p temper-production product_chat
cargo dev-clippy
cargo dev-check
```

Smith-side phases should run Smith's own fmt/tests and, when provider credentials
or Forgejo e2e prerequisites are available, the moved live/e2e gates. Keep these
commands documented in Smith's README as they are introduced. If a real-world
e2e gate cannot run locally, leave it ignored/env-gated and record what is
needed rather than deleting it.

## Relevant Temper starting points

- `crates/temper-agents/src/provider.rs`
- `crates/temper-agents/src/decision.rs`
- `crates/temper-agents/src/product_manager.rs`
- `crates/temper-agents/src/role.rs`
- `crates/temper-agents/src/registry.rs`
- `crates/temper-agents/tests/`
- `crates/temper-interaction/`
- `crates/temper-runner/src/agent.rs`
- `crates/temper-production/src/product_chat*.rs`
- `crates/temper-production/src/worker.rs`
- `crates/temper-testing/`
