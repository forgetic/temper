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

## Phase 5: Add pure queue evaluation and transition planning (done)

Added `plan` (`plan::Planner` / `ValidatedWorkflow::planner`): a deterministic, side-effect-free state machine over classified artifacts. `plan::matches_queue` and the `QueueQuery` trait match a `ClassifiedArtifact` against a `ValidatedQueue` or a compiled `QueueManifest`. `Planner::plan_transition` checks role authority, label preconditions (stale `remove_label`, contradicted `add_label`), required gates, and impossible resulting exclusive states, returning a `TransitionPlan` of typed `WorkflowEffect`s plus `Postcondition`s or a `PlanError` of `PlanDiagnostic`s. `WorkflowEffect` is the closed effect enum; only label effects are produced today, the rest are documented placeholders. Tests in `tests/planning.rs` cover queue selection, authority, stale/contradicted preconditions, gate enforcement, deterministic plans, and impossible-state diagnosis. No effects are applied to a backend.

## Phase 6: Execute transitions through `harness-forge` (done)

Added `execute` (`execute::Executor` / `ValidatedWorkflow::executor`), generic over `F: Forge + ?Sized`. `Executor::execute` loads fresh Forge state by item number, classifies it, re-plans the transition against that state (re-checking authority, preconditions, gates, and resulting states), applies the planned label effects as a single `update_issue`/`update_pull_request` call, and verifies the postconditions. `ExecutionError` distinguishes `Validation`, `Precondition`, `Backend`, `Classification`, `TargetMissing`, `UnsupportedEffect`, and `PostconditionFailed`. `Executor::ensure_issue` implements idempotent create by stamping a correlation key into the new issue's metadata block and searching existing issues for that key first; `EnsureOutcome` reports `Created` vs `Existing`. Tests in `tests/execution.rs` cover claim label updates, stale-precondition no-ops, idempotent create across retries, the three failure classes, the PR update path, and missing targets. No leases, journaling, or reconciliation yet; non-label effects are rejected as `UnsupportedEffect`.

## Phase 7: Add leases, journaling, and reconciliation

Add durable or abstract command journals, lease records, heartbeat handling, expired-lease recovery, and a reconciler for partial transitions and impossible states. Prefer traits so the filesystem backend can be used in deterministic tests.

## Phase 8: Add robustness and crash-injection tests

Add simulation and crash-injection tests that retry commands, duplicate tool calls, crash between effects, expire leases, and interleave multiple workers. Assert safety properties such as no duplicate PR per code issue and no merge before required gates pass.

## Phase boundaries

Keep each phase small. If a phase grows, stop after landing the lowest-level types and tests, then update this guide before handing off.

Do not expose broad Forge mutation tools to agents. Workflow tools should remain generated, role-specific, and transition-oriented.
