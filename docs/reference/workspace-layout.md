# Workspace layout

This page is a factual map of the repository. It complements the top-level README without carrying the detailed phase/status notes that belong in `AGENTS.md` and focused design documents.

## Workspace crates

| Path | Purpose |
| --- | --- |
| `crates/harness-forge/` | Provider-neutral Forge domain types and the async `Forge` interface for users, repositories, labels, issues, pull requests, comments, reviews, merges, dependency links, and CI jobs. |
| `crates/harness-forge-memory/` | In-memory reference backend used for fast workflow tests and deterministic local scenarios. It mirrors the filesystem backend's observable behavior. |
| `crates/harness-forge-filesystem/` | Local filesystem reference backend used for development, fixtures, and process-split tests. It serializes mutations with a store-level advisory lock. |
| `crates/harness-forge-forgejo/` | Forgejo provider backend. It implements the full `Forge` trait through Forgejo HTTP APIs, with mock HTTP contract tests by default and optional live smoke tests. |
| `crates/harness-workflow/` | Workflow/orchestration layer: typed specs, validation, artifact classification, relation handling, compilation, planning, execution, leases, journaling, reconciliation, and recovery application. |
| `crates/harness-runner/` | Backend-agnostic runner primitives: queue scans, `Agent` and `RoleTools`, role and mechanical workers, CI test seam, poll loop, trigger hint/coalescing primitives, drivers, and scenario stages. |
| `crates/harness-testing/` | Non-production testing support: deterministic fake agents, CI policies and sinks, fixtures, scenarios, the `harness-testing-worker` binary, and gated multi-process rehearsals. Production crates should not depend on it normally. |
| `crates/harness-agents/` | Real in-process LLM role agents using `pi_agent_rust`. This is the only crate that depends on the LLM SDK; workflow state mutations still go through `RoleTools`. |
| `crates/harness-production/` | Production-owned executable wiring: `harness-worker` for Forgejo role/mechanical workers with real agents and optional wake sockets, `harness-provision-forgejo` for demo/dev Forgejo provisioning and webhook registration, and `harness-trigger-forgejo` for webhook-to-local-wake triggering. It does not depend on `harness-testing`. |

## Other top-level directories

| Path | Purpose |
| --- | --- |
| `docs/` | Product documentation organized with Diátaxis: tutorials, how-to guides, reference pages, explanation pages, and ADRs. Start at `docs/README.md`. |
| `examples/reference-delivery/` | Operator-facing shell demo for the intended production topology. It is wired to the `harness-production` binary names and still needs live production-binary revalidation before it is a clean-checkout turnkey demo. |
| `plans/` | Implementation plans, roadmaps, and findings for larger work streams. These are useful for context, but stable behavior should also be captured in docs or code. |

## Where to look for details

- Forge contract: `docs/reference/forge-interface.md`.
- Backend behavior: `docs/reference/in-memory-backend.md`, `docs/reference/filesystem-backend.md`, and `docs/reference/forgejo-backend.md`.
- Workflow runtime contract: `docs/reference/workflow-layer.md` and `docs/reference/robustness-guarantees.md`.
- Conceptual overview: `docs/explanation/agentic-workflows.md`.
- End-to-end scenarios: `docs/how-to/run-reference-delivery-end-to-end.md` and `docs/how-to/run-forgejo-multiprocess-e2e.md`.
