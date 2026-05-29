# Implement the workflow layer in phases

Use this plan to split `harness-workflow` implementation across fresh agent sessions. Each phase should begin with the normal session bootstrap in `AGENTS.md` and end with `cargo fmt --all` and `cargo dev-check`.

Read first:

- `docs/adr/0007-workflow-layer-and-agent-compilation.md`
- `docs/explanation/agentic-workflows.md`
- `docs/reference/workflow-layer.md`

## Phase 1: Rename the crate

Rename `crates/harness-core` to `crates/harness-workflow` and update Cargo metadata, README files, ADR cross-references, docs indexes, and crate-level docs. Do not add workflow functionality yet.

## Phase 2: Add spec and validation foundations

Create focused modules for raw workflow specs, typed IDs, validation diagnostics, and `ValidatedWorkflow`. Implement static checks for duplicate IDs and missing references. Add unit tests for success and failure cases.

## Phase 3: Model artifacts, labels, and metadata

Add artifact-kind mappings, state dimensions, label projections, and metadata block parsing/rendering for workflow kind, relations, correlation keys, and leases. Add classifiers that turn Forge issues/PRs into workflow artifacts or invariant violations.

## Phase 4: Compile role, prompt, tool, and label manifests

Compile a validated workflow into role manifests, prompt manifests, role-specific tool manifests, queue manifests, and label manifests. Include a checked-in five-role workflow fixture and tests for generated manifests.

## Phase 5: Add pure queue evaluation and transition planning

Implement queue matching over classified artifacts and pure transition planning. Produce typed workflow effects without applying them to a backend. Test state-machine behavior without Forge side effects.

## Phase 6: Execute transitions through `harness-forge`

Add an executor that applies planned effects through the `Forge` trait. Enforce fresh-state preconditions before mutation. Implement idempotent create behavior with correlation keys where possible. Test with `harness-fs`.

## Phase 7: Add leases, journaling, and reconciliation

Add durable or abstract command journals, lease records, heartbeat handling, expired-lease recovery, and a reconciler for partial transitions and impossible states. Prefer traits so the filesystem backend can be used in deterministic tests.

## Phase 8: Add robustness and crash-injection tests

Add simulation and crash-injection tests that retry commands, duplicate tool calls, crash between effects, expire leases, and interleave multiple workers. Assert safety properties such as no duplicate PR per code issue and no merge before required gates pass.

## Phase boundaries

Keep each phase small. If a phase grows, stop after landing the lowest-level types and tests, then update this guide before handing off.

Do not expose broad Forge mutation tools to agents. Workflow tools should remain generated, role-specific, and transition-oriented.
