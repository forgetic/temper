# Agent entry point

This file is the first stop for coding agents working in Harness. It should be changed only if the codebase map changes.
Keep it as an orientation map only; stable process rules and detailed status live in the linked docs.
For every session, read `README.md` first and docs/README.md next.

## Codebase map

Rows are ordered by the mental model agents usually need: Forge contract and backends, workflow runtime, runner/test support, then real-agent and deployable wiring.

| Path | Look here for |
| --- | --- |
| `crates/harness-forge/` | Portable Forge model and `Forge` trait; start here for provider-neutral issue, PR, review, dependency, CI, and change-hint API changes. |
| `crates/harness-forge-memory/` | Fast in-memory reference backend for deterministic workflow tests and local scenarios. |
| `crates/harness-forge-filesystem/` | Persistent local reference backend for fixtures, local stores, and multi-process/process-split tests. |
| `crates/harness-forge-forgejo/` | Forgejo HTTP backend, provider-specific mapping, optional live smoke tests, and offline mock-contract tests. |
| `crates/harness-workflow/` | Workflow definitions and runtime logic: validation, classification, compilation, planning/execution, leases, reconciliation, and recovery. |
| `crates/harness-runner/` | Backend-agnostic worker runtime: queue scans, `RoleTools`, role/mechanical workers, polling/wake hints, multi-repo scans, runner config, and external-tool seams such as `coding_workspace`. |
| `crates/harness-testing/` | Non-production fakes, fixtures, scenario drivers, CI sinks, the testing worker binary, and gated e2e rehearsals. |
| `crates/harness-agents/` | Real in-process LLM role agents and provider/auth wiring; keep pi SDK usage here. |
| `crates/harness-production/` | Deployable Forgejo binaries: workers, provisioning, webhook trigger, product-manager chat, and production external-tool bindings. |
| `examples/reference-delivery/` | Operator-facing reference-delivery demo and launch scripts. |
| `plans/` | Roadmaps and findings; promote stable behavior into `docs/` before relying on it. |
