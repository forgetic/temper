# Harness

Harness is a Rust workspace for building agentic workflows on top of Forge-like collaboration platforms such as Forgejo. It treats issues, pull requests, labels, comments, reviews, dependency links, CI results, and workflow metadata as the durable source of truth, then coordinates agents through a typed workflow runtime.

The project is intentionally backend-agnostic: workflow policy lives above a portable `Forge` interface, while concrete backends adapt that interface to memory, the local filesystem, Forgejo, or future providers.

## What is in this repository

Harness has four main layers:

- **Forge interface and backends** define the collaboration-domain model and provider adapters.
- **Workflow runtime** validates workflow specs, classifies Forge artifacts, plans transitions, executes authorized effects, and repairs interrupted work through leases, journaling, and reconciliation.
- **Runner primitives** scan active queues, dispatch role work to agents, expose role-scoped tools, and run mechanical controller work.
- **Agents and test harnesses** provide deterministic fake agents for reproducible scenarios and real in-process LLM agents for Forgejo end-to-end rehearsals.

The core workflow path is implemented and covered by deterministic memory/filesystem tests. The Forgejo backend is implemented and tested offline by default, with gated live and multi-process end-to-end tests for real Forgejo, real CI, and optional real LLM agents. The operator-facing demo in `examples/reference-delivery/` now targets production-owned binaries in `harness-production`; it still needs live revalidation and is not yet a turnkey deployment.

## Workspace map

```text
crates/
  harness-forge/            Portable Forge domain model and async interface.
  harness-forge-memory/     In-memory reference backend for fast tests.
  harness-forge-filesystem/ Filesystem reference backend for local and process-split tests.
  harness-forge-forgejo/    Forgejo HTTP backend with offline contract tests.
  harness-workflow/         Workflow spec, validation, planning, execution, and recovery.
  harness-runner/           Scanner, workers, role tools, drivers, and poll loop primitives.
  harness-testing/          Non-production fake agents, fixtures, scenarios, and worker binary.
  harness-agents/           Real LLM role agents behind the same runner `Agent` boundary.
  harness-production/       Production `harness-worker` and `harness-provision-forgejo` binaries.

docs/                       Diátaxis documentation: tutorials, how-to, reference, explanation, ADRs.
examples/reference-delivery/ Shell demo for the intended production topology.
plans/                      Implementation plans and findings for larger work streams.
```

For a more detailed crate inventory, see `docs/reference/workspace-layout.md`.

## Getting started as a developer

Use the fast workspace aliases from `.cargo/config.toml`:

```sh
cargo dev-check       # cargo check --workspace --all-targets
cargo fmt --all
cargo dev-clippy
```

When behavior changes, run the relevant tests; `cargo dev-test` runs the workspace test suite. Default tests are intended to be hermetic and should not contact live services. Live Forgejo and real-agent scenarios are opt-in through the documented environment gates.

Useful next reads:

- `docs/README.md` — documentation map.
- `docs/how-to/fast-local-iteration.md` — local validation loop.
- `docs/how-to/run-reference-delivery-end-to-end.md` — deterministic reference scenarios.
- `docs/how-to/run-forgejo-multiprocess-e2e.md` — gated real Forgejo/CI rehearsals.
- `docs/explanation/agentic-workflows.md` — the conceptual model.

## For coding agents

Autonomous coding agents should start with `AGENTS.md`. It contains the detailed current repository state, operating rules, validation expectations, and links to the agent lessons register.
