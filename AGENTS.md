# Agent entry point

This file is the first stop for coding agents working in Temper. It should be changed only if the codebase map or documentation map changes.
Keep it as an orientation map only; stable process rules and detailed status live in the linked docs.
For every session, read `README.md` first and then open the task-relevant docs below.

## Codebase map

Rows are ordered by the mental model agents usually need: Forge contract and backends, workflow runtime, runner/test support, then process-responder and deployable wiring.

| Path | Look here for |
| --- | --- |
| `crates/temper-forge/` | Portable Forge model and `Forge` trait; start here for provider-neutral issue, PR, review, dependency, CI, and change-hint API changes. |
| `crates/temper-process-protocol/` | Serialization-only JSON DTOs and validation helpers for external workflow-role and interactive responder processes. |
| `crates/temper-interaction/` | Provider-neutral interactive conversation domain types, inert proposals, and responder trait; keep transport, workflow, Forge mutation, and LLM-provider wiring out. |
| `crates/temper-forge-memory/` | Fast in-memory reference backend for deterministic workflow tests and local scenarios. |
| `crates/temper-forge-filesystem/` | Persistent local reference backend for fixtures, local stores, and multi-process/process-split tests. |
| `crates/temper-forge-forgejo/` | Forgejo HTTP backend, provider-specific mapping, optional live smoke tests, and offline mock-contract tests. |
| `crates/temper-workflow/` | Workflow definitions and runtime logic: validation, classification, compilation, planning/execution, leases, reconciliation, and recovery. |
| `crates/temper-runner/` | Backend-agnostic worker runtime: queue scans, `RoleTools`, role/mechanical workers, polling/wake hints, multi-repo scans, runner config, and external-tool seams such as `coding_workspace`. |
| `crates/temper-testing/` | Non-production fakes, fixtures, scenario drivers, CI sinks, worker logic, and gated e2e rehearsals. |
| `src/bin/` | Workspace root `temper` package binary wiring for deployable tools; thin entrypoints delegate into focused runtime crates. |
| `crates/temper-reference-delivery/` | Lightweight reference-delivery workflow, repository, actor, and runner defaults shared by deployable tools. |
| `crates/temper-worker/` | Forgejo-backed production worker wiring for role and mechanical workers. |
| `crates/temper-wake/` | Host-local authenticated wake socket bus shared by worker, trigger, and tests. |
| `crates/temper-trigger-forgejo/` | Forgejo webhook receiver that verifies payloads and emits wake hints. |
| `crates/temper-forgejo-provision/` | Forgejo provisioning and seeding for the reference-delivery demo. |
| `crates/temper-forgejo-ops/` | Low-level Forgejo REST helpers outside the portable `Forge` trait. |
| `crates/temper-coding-workspace/` | Local-git `coding_workspace` provider and PR diff safety helpers. |
| `crates/temper-interaction-service/` | Deployable REPL/HTTP interaction service, deployment bindings, args, DTOs, and transport glue. |
| `crates/temper-reference-delivery-validator/` | Operator-facing reference-delivery validator. |
| `examples/reference-delivery/` | Operator-facing reference-delivery demo and launch scripts. |

## Documentation map

- Development rules and validation: `docs/reference/development-conventions.md`, `docs/how-to/fast-local-iteration.md`, and `docs/how-to/end-a-development-session.md`.
- Contracts: `docs/reference/forge-interface.md`, `docs/reference/in-memory-backend.md`, `docs/reference/filesystem-backend.md`, `docs/reference/forgejo-backend.md`, `docs/reference/workflow-layer.md`, `docs/reference/cross-repo-workflows.md`, and `docs/reference/robustness-guarantees.md`.
- Concepts: `docs/explanation/agentic-workflows.md`, `docs/explanation/domain-model.md`, `docs/explanation/cross-repo-workflows.md`, `docs/explanation/reference-workflow.md`, and `docs/explanation/forgejo-e2e-topology.md`.
- LLM agent details: `docs/reference/llm-agents.md`, `docs/how-to/use-chatgpt-oauth-auth.md`, and `docs/how-to/configure-coding-workspace.md`.
- Significant decisions: `docs/adr/README.md`. The full documentation index is `docs/README.md`.
