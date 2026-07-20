# Codebase map

Temper is a Rust workspace split by process plane and by reusable contract. Use
this page as a navigation map; `Cargo.toml` remains the exact source of truth for
workspace membership.

Rows are ordered by the mental model agents usually need: entrypoints and
operator surfaces, Forge contracts and backends, workflow/control runtime,
worker/agent boundaries, interaction surfaces, then testing and repository
support.

## Entry points and operator surfaces

| Path | Look here for |
| --- | --- |
| `src/bin/temper.rs` | The unified `temper` binary composition root. It snapshots argv/env/paths/cwd once and delegates to `temper-cli`. |
| `crates/temper-cli/` | Thin dispatcher for the public operator lifecycle `temper init -> temper check -> temper plan -> temper apply -> temper serve`, plus `config` inspection and hidden compatibility/internal entry points. |
| `crates/temper-cli-common/` | Shared CLI plumbing: prompt abstraction, argv helpers, exit-code wrappers, file target resolution, and terminal-safe file writes. |
| `crates/temper-cli-config/` | `temper config` inspection helpers: check/validate, show, paths, schema, and starter template output over `temper-config`. |
| `crates/temper-cli-init/` | Interactive `temper init`, `temper plan`, and explicit `temper apply`: collect operator answers, write config/workflow/credential artifacts, derive/preview provisioning plans, and optionally mutate Forgejo. |
| `crates/temper-cli-daemon/` | `temper serve` wiring for standalone all-in-one mode or individual engine/worker services, including engine/standalone `POST /forgejo/webhook` intake; legacy `temper daemon` dispatch remains here for compatibility. There is no runnable `serve trigger` component. |
| `crates/temper-engine-service/` | Slim `temper-engine` service binary and adapters from resolved config into a running engine daemon. |
| `crates/temper-worker-service/` | Slim `temper-worker` service binary and adapters from resolved config into a long-polling worker that spawns out-of-process agents. |
| `crates/temper-agent-session/` | Slim `temper-agent` process boundary: read `WorkspaceContext`, run one native coding-agent session in the prepared workspace, write `WorkspaceResult`. |
| `crates/temper-web/` | Web dashboard: Rust HTTP/SSE server, read model, feed adapters, and bundled TypeScript UI under `ui/`. |
| `crates/temper-interaction-service/` | Deployable REPL/HTTP interaction service, args, bindings, DTOs, and transport glue for interactive profiles. |
| `crates/temper-trigger-forgejo/` | Legacy/internal Forgejo webhook receiver that verifies payloads and emits authenticated wake hints for wake-socket fixtures/older topologies; not a `temper serve` process. Supported operator webhook intake is the engine/standalone `/forgejo/webhook` route. |
| `crates/temper-scenario-cli/` | Standalone `temper-scenario` facade for listing, checking, running, validation-report bridging, and promotion-draft scaffolding for executable scenario manifests. |
| `crates/temper-benchmark-cli/` | Standalone `temper-benchmark` facade for trace analysis, direct harness/live agent sessions, repetition aggregation, and report-only artifact comparison. See the [benchmark guide](../how-to/benchmark-agent-sessions.md). |

## Configuration, provisioning, and reference delivery

| Path | Look here for |
| --- | --- |
| `crates/temper-config/` | Schema-based `config.toml`/`credentials.toml` loading, secret-source handling, environment/path abstraction, JSON Schema, and resolved runtime settings. |
| `crates/temper-reference-delivery/` | Lightweight reference-delivery defaults: bundled workflow fixture, repository inputs, role actor mapping, and runner defaults shared by deployable tools. |
| `crates/temper-provision/` | Backend-agnostic provisioning orchestration over the Forge capability traits: owners/users/tokens/access, repositories, labels, seed content, CI, webhooks, and optional intake issue. |
| `crates/temper-provision-forgejo-cli/` | Demo/operator `temper provision-forgejo` implementation: builds a Forgejo backend, runs `temper-provision`, seeds the reference intake issue, and writes demo secrets. |
| `crates/temper-reference-delivery-validator/` | Operator-facing validator for reference-delivery Forge state. |
| `crates/temper-scenario-core/` | Scenario manifest structs, TOML parsing, discovery, and diagnostics shared by `temper-scenario`. |

## Forge contract and backends

| Path | Look here for |
| --- | --- |
| `crates/temper-forge-model/` | Backend-agnostic Forge domain model, `Forge` trait, IDs, change hints, and provisioning capability traits (`ForgeContent`, `ForgeAdmin`). |
| `crates/temper-forge/` | Top-level Forge facade: re-exports the model and owns backend factory helpers. This is the only non-test crate that should depend directly on concrete backends. |
| `crates/temper-forge-memory/` | Fast in-memory reference backend for deterministic workflow tests and local scenarios. |
| `crates/temper-forge-filesystem/` | Persistent local reference backend for fixtures, local stores, concurrency tests, and multi-process/process-split rehearsals. |
| `crates/temper-forge-forgejo/` | Forgejo HTTP backend, provider-specific DTO/mapping code, CI/web-UI scraping support, provisioning helpers, offline mock-contract tests, and optional live tests. |
| `crates/temper-forge-github/` | GitHub HTTP backend, provider-specific DTO/mapping code, and hermetic mock-contract tests. |

## Workflow, engine, and control-plane runtime

| Path | Look here for |
| --- | --- |
| `crates/temper-workflow/` | Workflow specifications, validation, classification, compilation, planning/execution, gates, leases, reconciliation, recovery, and workflow metadata handling. |
| `crates/temper-runner/` | Backend-agnostic runner primitives: queue scans, `RoleTools`, role/mechanical workers, polling/wake hints, multi-repo scans, runner config, and the `coding_workspace` external-tool seam. |
| `crates/temper-engine/` | Orchestrator daemon: worker/daemon protocol handling, job feed, result appliers, Forge-backed result application, lease-gated mutation, mechanical backstop, webhook/local wake wiring, and PR freshness checks. |
| `crates/temper-worker-registry/` | In-memory worker scheduling registry used by the daemon as a soft hint for worker capabilities, health, and in-flight capacity. |
| `crates/temper-daemon-transport/` | Tiny in-process worker-to-daemon transport glue for co-resident daemon/worker stacks, simulations, and hermetic tests. |
| `crates/temper-engine-io/` | Completion-engine shell for daemon/control services: pure `Machine` contract, drive loop, completion queues, and HTTP/timer/process/cadence executors; see `io-engine-architecture.md`. |
| `crates/temper-wake/` | Host-local authenticated wake socket bus shared by worker, legacy/internal trigger fixtures, and tests. |
| `crates/temper-log/` | Process-wide logging initialization and the structured event model used by engine, worker, agent, and the `trigger` service plane for inbound facts. |
| `crates/temper-sim/` | Deterministic simulation harness that runs production daemon/worker code under skein's lab runtime with virtual time and reproducible schedules. |

## Worker, agent, and process protocols

| Path | Look here for |
| --- | --- |
| `crates/temper-protocol-worker/` | Serde-only Worker/Daemon wire protocol DTOs: registration, polling, job assignment, workspace manifests, lifecycle, and job results. |
| `crates/temper-worker/` | Orchestration worker library: long-poll daemon client, job execution, coding workspace preparation, git/pre-push checks, out-of-process agent runner, and worker state machine/shell. |
| `crates/temper-worker-io/` | Worker-local completion engine for the worker state machine and shell. It is intentionally separate from `temper-engine-io`. |
| `crates/temper-protocol-agent/` | Serde-only Worker/Agent process protocol: `WorkspaceContext` in, optional live `submit_for_pr` side channel, `WorkspaceResult` out. |
| `crates/temper-agent/` | Temper-specific agent/provider core: coding-agent execution, provider/auth selection, prompt overlays, product-manager replies, structured decision parsing, and usage accounting. |
| `crates/temper-agent-core/` | Sans-IO LLM agent loop: pure `AgentMachine`, streaming/tool request protocol, sub-agent support, and shell integration. |
| `crates/temper-agent-io/` | Agent-local completion engine and HTTP-client/timer runtime helpers. It is intentionally separate from the engine and worker IO crates. |

## Interactive conversation plane

| Path | Look here for |
| --- | --- |
| `crates/temper-protocol-interaction/` | Serde-only JSON conversation/proposal protocol and validation helpers for external interactive responder processes. |
| `crates/temper-interaction/` | Provider-neutral interaction domain: profile validation/compilation, Forge-backed transcripts, proposal state, manifest-driven acceptance, responder traits, and process responder adapter. |
| `docs/explanation/interactive-agent-interfaces.md` | Conceptual boundary between workflow role agents and interactive human-facing profiles. |

## Testing, demos, and repository support

| Path | Look here for |
| --- | --- |
| `crates/temper-testing/` | Non-production fakes, fixtures, fake agents/CI, Forgejo runtime/server helpers, real-stack harnesses, worker binaries, and shared scenario drivers. |
| `crates/temper-dev/` | Tiny repo-local developer driver used by Cargo aliases that need to sequence multiple commands, such as `cargo dev-test-full`. |
| `tests/` | Root integration and e2e tests for the unified CLI, config paths/schema/show, Forgejo daemon/init flows, and in-process transport. |
| `examples/basic-delivery/` | Operator-facing basic-delivery demo config, workflow, CI file, observability notes, and launch script. |
| `examples/reference-delivery/` | Operator-facing reference-delivery demo config, workflow, CI file, observability notes, and launch script. |
| `scenarios/` | Checked-in declarative validation corpus: scenario manifests, fixture inputs, minimal repo seeds, and authoring guidance for promoted post-merge validation cases. |
| `benchmarks/` | Separate [agent-session benchmark corpus](../../benchmarks/README.md): fixed direct-agent fixtures and manifests for repetition, structural measurement, and performance reporting. |
| `.cargo/config.toml` | Developer Cargo aliases (`dev-check`, `dev-test-quick`, `dev-test-full`, `dev-benchmark-harness`, `dev-doc`) and local sibling-repo patch guidance. |
| `.forgejo/workflows/ci.yml` | Forgejo CI workflow for Rust validation and the web UI lane. |
| `docs/adr/` | Architecture decision records. Start here for historical rationale behind backend boundaries, workflow semantics, triggering, native Forge state, and multi-repo work. |
| `docs/explanation/` | Conceptual explanations like this map, the domain model, agentic workflows, process split, Forgejo topology, logging/observability, and IO engine architecture. |
| `docs/reference/` | Contract/reference material: Forge interface, workflow layer/runtime/spec, worker-daemon protocol, interactive protocol, backends, testing inventory, and environment variables. |
| `docs/how-to/` | Task-oriented contributor/operator guides for running demos, daemon e2es, coding workspaces, fast local iteration, OAuth, and test-writing. |
