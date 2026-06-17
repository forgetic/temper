# Codebase map

Rows are ordered by the mental model agents usually need: Forge contract and backends, workflow runtime, runner/test support, then process-responder and deployable wiring.

| Path | Look here for |
| --- | --- |
| `crates/temper-forge/` | Portable Forge model and `Forge` trait; start here for provider-neutral issue, PR, review, dependency, CI, and change-hint API changes. |
| `crates/temper-protocol-interaction/` | Serialization-only JSON DTOs and validation helpers for external interactive (conversation/proposal) responder processes. |
| `crates/temper-protocol-decision/` | Serialization-only JSON DTOs and validation helpers for external workflow-role decision responder processes. |
| `crates/temper-interaction/` | Provider-neutral interactive conversation domain types, inert proposals, and responder trait; keep transport, workflow, Forge mutation, and LLM-provider wiring out. |
| `crates/temper-forge-memory/` | Fast in-memory reference backend for deterministic workflow tests and local scenarios. |
| `crates/temper-forge-filesystem/` | Persistent local reference backend for fixtures, local stores, and multi-process/process-split tests. |
| `crates/temper-forge-forgejo/` | Forgejo HTTP backend, provider-specific mapping, optional live smoke tests, and offline mock-contract tests. |
| `crates/temper-forge-github/` | GitHub HTTP backend, provider-specific mapping, and offline mock-contract tests (first pass: hermetic tests only, no native dependency links). |
| `crates/temper-workflow/` | Workflow definitions and runtime logic: validation, classification, compilation, planning/execution, leases, reconciliation, and recovery. |
| `crates/temper-engine-io/` | io_uring-style completion engine on the asupersync runtime: the `Machine` functional-core contract, the `drive` loop, completion queues, and HTTP/timer/process/cadence executors; see `docs/explanation/io-engine-architecture.md`. |
| `crates/temper-runner/` | Backend-agnostic worker runtime: queue scans, `RoleTools`, role/mechanical workers, polling/wake hints, multi-repo scans, runner config, and external-tool seams such as `coding_workspace`. |
| `crates/temper-testing/` | Non-production fakes, fixtures, scenario drivers, CI sinks, worker logic, and gated e2e rehearsals. |
| `src/bin/` | Workspace root `temper` package binary wiring for deployable tools; thin entrypoints delegate into focused runtime crates. |
| `crates/temper-reference-delivery/` | Lightweight reference-delivery workflow, repository, actor, and runner defaults shared by deployable tools. |
| `crates/temper-wake/` | Host-local authenticated wake socket bus shared by worker, trigger, and tests. |
| `crates/temper-trigger-forgejo/` | Forgejo webhook receiver that verifies payloads and emits wake hints. |
| `crates/temper-provision-forgejo-cli/` | Reference-delivery demo / operator CLI for `temper provision-forgejo`: builds a ForgejoForge and runs the backend-agnostic `temper-provision` orchestration, seeds the demo intake issue, writes secrets.env. |
| `crates/temper-forgejo-ops/` | Low-level Forgejo REST helpers outside the portable `Forge` trait. |
| `crates/temper-interaction-service/` | Deployable REPL/HTTP interaction service, deployment bindings, args, DTOs, and transport glue. |
| `crates/temper-reference-delivery-validator/` | Operator-facing reference-delivery validator. |
| `examples/reference-delivery/` | Operator-facing reference-delivery demo and launch scripts. |
