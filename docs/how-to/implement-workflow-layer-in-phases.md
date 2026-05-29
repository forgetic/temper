# Implement the workflow layer in phases

Use this plan to split `harness-workflow` implementation across fresh agent sessions. Each phase should begin with the normal session bootstrap in `AGENTS.md` and end with `cargo fmt --all` and `cargo dev-check`.

Read first:

- `docs/adr/0007-workflow-layer-and-agent-compilation.md`
- `docs/explanation/agentic-workflows.md`
- `docs/reference/workflow-layer.md`

## Phase 1: Rename the crate (done)

Renamed `crates/harness-core` to `crates/harness-workflow` and updated Cargo metadata, README files, AGENTS, ADR cross-references, docs indexes, and crate-level docs. No workflow functionality was added in this phase.

## Phase 2: Add spec and validation foundations (done)

Added focused modules in `crates/harness-workflow/src`: `ids` (typed ids), `spec` (`RawWorkflowSpec` and raw children), `diagnostics` (`Diagnostic`, `Severity`, `SymbolKind`, `ReferenceSite`, `ValidationErrors`), `validated` (`ValidatedWorkflow` with a crate-private constructor), and `validate` (diagnostic-collecting static validation). Implemented checks for duplicate ids and undeclared references (roles, labels, artifact kinds, queues, transitions, gates). Integration tests in `tests/validation.rs` cover valid workflows, duplicate ids, missing references across every site, serde loading, and the `ValidatedWorkflow`-only API shape.

## Phase 3: Model artifacts, labels, and metadata (done)

Added `artifact::ArtifactTarget` (artifact kinds now map to a Forge issue or PR via `target` and carry `identifying_labels`), an `exclusive` flag on state dimensions, `metadata::WorkflowMetadata`/`Lease` with JSON-in-HTML-comment render/parse, and `classify::Classifier` that turns a `harness_forge::Issue` or `PullRequest` into a `ClassifiedArtifact` or a `ClassificationError` of `ClassificationDiagnostic`s. `harness-workflow` now depends on `harness-forge`. Tests in `tests/metadata.rs` and `tests/classification.rs` cover round-trips, label-based classification, exclusive conflicts, and missing/malformed metadata.

## Phase 4: Compile role, prompt, tool, and label manifests (done)

Added `compile` (`compile::compile` / `ValidatedWorkflow::compile`) producing a `CompiledWorkflow` with `RoleManifest` (id, charter, concurrency hint, subscribed queues, transition authority, role-specific tools, and an embedded `PromptManifest`), `ToolManifest` (intent-level, one per authorized transition), `QueueManifest` (with subscribers), `LabelManifest`/`LabelSpec`/`LabelUsage`, and a `TransitionManifest` runtime table. Added a `concurrency` hint to `RawRole`/`ValidatedRole`. Checked in `crates/harness-workflow/fixtures/five-role-delivery.json` (architect, engineer, reviewer, tester, owner) and `tests/compilation.rs` covering fixture validation, per-role manifests, role-scoped tools/authority, label coverage across artifact/state/queue/gate sites, and deterministic prompts. No transitions are executed.

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
