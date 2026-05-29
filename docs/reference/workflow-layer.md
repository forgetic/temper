# Workflow layer reference

This page defines the intended contract for the `harness-workflow` crate. Phases 2–5 have landed spec validation, artifact/metadata modeling, classification, compilation to manifests, and pure queue evaluation and transition planning; the rest of the contract below (runtime execution, recovery) is still planned. See "Implementation status" for what exists today.

## Scope

`harness-workflow` owns workflow policy and orchestration on top of `harness-forge`. It must not contain concrete Forge backend code or agent-provider code.

The crate should provide:

- workflow specification types
- validation from raw specs to `ValidatedWorkflow`
- compilation to role prompts, tool manifests, and label manifests
- queue evaluation and transition planning
- runtime transition enforcement through the `Forge` trait
- idempotency, leases, journaling, reconciliation, and test helpers

## Type phases

Runtime and compiler APIs must not accept an unvalidated specification.

Recommended phases:

- `RawWorkflowSpec`: loaded from YAML, JSON, TOML, or generated input
- `ValidatedWorkflow`: normalized, internally consistent workflow
- `CompiledWorkflow`: role prompts, tool manifests, label manifests, and runtime tables
- `RuntimeWorkflow`: compiled workflow plus backend handles and durable state

Validation errors should be diagnostic collections so users can fix multiple spec issues at once.

## Implementation status

Phase 2 implemented the spec and validation foundations:

- `spec::RawWorkflowSpec` and its raw child structs (`RawRole`, `RawLabel`, `RawArtifactKind`, `RawStateDimension`, `RawState`, `RawQueue`, `RawTransition`, `RawEffect`, `RawGate`) load from serde input.
- `validated::ValidatedWorkflow` is the normalized model. It has no public constructor; the only way to build one is `RawWorkflowSpec::validate` / `validate::validate`, so compiler and runtime APIs added later can require it by type.
- `ids` provides typed ids: `RoleId`, `LabelId`, `ArtifactKindId`, `StateDimensionId`, `StateId`, `QueueId`, `TransitionId`, `GateId`.
- `diagnostics` provides `Diagnostic`, `Severity`, `SymbolKind`, `ReferenceSite`, and the `ValidationErrors` collection.

Phase 3 added artifact/Forge mapping, metadata blocks, and classification:

- `artifact::ArtifactTarget` maps each artifact kind to a Forge `Issue` or `PullRequest`. Artifact kinds now carry `target` and `identifying_labels`.
- State dimensions carry an `exclusive` flag (default `true`) so the classifier can reject impossible label combinations.
- `metadata::WorkflowMetadata` (with `Lease`) is the machine-readable block parsed from and rendered into artifact bodies. See "Metadata block format".
- `classify::Classifier` turns a `harness_forge::Issue` or `PullRequest` into a typed `ClassifiedArtifact`, or a `ClassificationError` carrying `ClassificationDiagnostic`s. See "Artifact classification".
- `harness-workflow` now depends on `harness-forge` because classification consumes Forge domain types.

Not yet modeled: `relation` as a first-class spec primitive (only metadata relations exist), `invariant`, `recovery_policy`, and runtime execution. Effects still cover only label add/remove.

Gate/transition wiring is modeled in both directions: a transition lists `requires_gates`, and a gate lists `satisfied_by` transitions.

Phase 4 added compilation:

- `compile::compile` (also `ValidatedWorkflow::compile`) projects a validated workflow into a `CompiledWorkflow`. Compilation is infallible because it consumes an already-validated workflow.
- `RoleManifest` carries role id, charter, concurrency hint, subscribed queues, transition `authority`, role-specific `tools`, and a `PromptManifest`.
- `ToolManifest` is an intent-level operation (named after its transition) carrying artifact, required gates, and effects. Tools are derived from the transitions a role is authorized for, so a role can never see a tool outside its authority.
- `QueueManifest` adds `subscribers` to each queue; `TransitionManifest` is the runtime transition table; `LabelManifest`/`LabelSpec`/`LabelUsage` enumerate every label a workflow site needs and why.
- `PromptManifest`/`PromptSection` hold deterministic prompt sections (`Role`, `Charter`, `Queues`, `Authorized actions`) with a stable `render` method.
- Roles now carry an optional `concurrency` hint (`RawRole`/`ValidatedRole`), compiled into the role manifest.

Not yet implemented from compilation: generated tool bodies and optional generated Rust code.

Phase 5 added pure queue evaluation and transition planning:

- `plan::Planner` (also `ValidatedWorkflow::planner`) borrows a `ValidatedWorkflow` and never touches a Forge backend. It matches classified artifacts against queues and plans transitions into typed effects.
- `plan::matches_queue` matches a `ClassifiedArtifact` against any `QueueQuery`. `QueueQuery` is implemented by both `ValidatedQueue` and the compiled `QueueManifest`, so the same matcher serves the validated model and a compiled runtime table.
- `plan::WorkflowEffect` is the closed planning-effect enum. `plan::Postcondition` carries the conditions that must hold after a plan applies. `plan::TransitionPlan` bundles transition, role, artifact kind, target, effects, and postconditions.
- `plan::PlanError` collects `plan::PlanDiagnostic`s (unauthorized role, artifact-kind mismatch, stale/contradicted label preconditions, unsatisfied gates, and impossible resulting states).

Not yet implemented: executing plans against a backend, idempotent create behavior, leases, journaling, and reconciliation.

## Spec primitives

A workflow spec contains these logical primitives.

| Primitive | Meaning |
| --- | --- |
| `role` | Actor authority, queues, concurrency limits, and prose charter |
| `artifact_kind` | Logical item with a Forge `target` (issue or PR) and `identifying_labels` |
| `state_dimension` | Named state group with an `exclusive` flag, projected as labels |
| `queue` | Query that selects artifacts needing attention |
| `transition` | Guarded state change authorized for one or more roles |
| `gate` | Condition that unlocks another transition, such as merge readiness |
| `relation` | Typed link between artifacts, such as parent, dependency, or produced PR |
| `invariant` | Condition that must hold during runtime scans |
| `recovery_policy` | What to do with expired leases, partial transitions, and drift |

Labels are a portable Forge projection of workflow state. The workflow layer may use metadata blocks in bodies or comments for information that is not represented by the current Forge interface, such as correlation keys and typed relations.

## Static validation

Validation must reject or diagnose:

- duplicate role, label, artifact, state-dimension, queue, transition, or gate IDs (implemented; state ids are checked for uniqueness within each dimension)
- references to undeclared roles, labels, artifact kinds, queues, transitions, or gates (implemented)
- transitions whose effects contradict declared mutually exclusive dimensions (planned)
- gates that cannot be satisfied by any declared transition (planned)
- role tool declarations that exceed the role's transition authority (planned)
- artifact mappings that cannot be represented by the Forge interface (planned)

Validation should also warn about unreachable queues, terminal states with no explanation, and labels that are declared but unused (planned).

In the current model, labels are the only cross-referenced projection of state: artifact mappings, queues, state declarations, and transition effects all reference label ids. States are not referenced by id outside their dimension, so undeclared-state references are not a current diagnostic.

## Metadata block format

Workflow information that has no portable Forge field lives in a metadata block embedded in an issue or pull-request body. The block is JSON wrapped in an HTML comment:

```text
<!-- harness:workflow
{
  "kind": "code",
  "parents": [12],
  "dependencies": [34],
  "correlation_key": "code-issue-42",
  "lease": {
    "role": "engineer",
    "worker": "run-abc",
    "claimed_at": "2026-05-29T00:00:00Z",
    "heartbeat_at": "2026-05-29T00:05:00Z",
    "expires_at": "2026-05-29T00:30:00Z"
  }
}
-->
```

JSON-in-an-HTML-comment is deliberate: it renders invisibly in Forge markdown, needs no parser beyond `serde_json` (no YAML/TOML dependency), and serializes deterministically because field order follows the struct declaration and empty fields are omitted. The block ends at the first `-->`, so values must not contain that sequence.

`render_metadata_block` and `parse_metadata_block` are inverses. Parsing returns `Ok(None)` when no block is present, `Ok(Some(_))` when one parses, and `Err(MetadataError)` when a block is present but unterminated or contains invalid JSON. Relations (`parents`, `dependencies`) are Forge item numbers in the same repository, matching the shared issue/PR number namespace.

## Artifact classification

`Classifier::classify_issue` and `Classifier::classify_pull_request` interpret a Forge artifact under a `ValidatedWorkflow`. Classification reads labels and the metadata block; it never mutates Forge state.

Kind resolution: when metadata names a `kind`, that kind is authoritative. Otherwise the kind is inferred from `identifying_labels` — a kind matches when all of its identifying labels are present, and the most specific match (most identifying labels) wins.

State resolution: for each dimension, the active states are those whose label is present. An exclusive dimension with more than one active state is an impossible combination.

Success yields a `ClassifiedArtifact` (kind, target, source, per-dimension states, parsed metadata, raw labels). Otherwise a `ClassificationError` collects every `ClassificationDiagnostic`:

- `Unclassified`: no kind matched and metadata named none
- `AmbiguousArtifactKind`: several kinds matched equally well
- `UnknownMetadataKind`: metadata named an undeclared kind
- `TargetMismatch`: the kind maps to a different Forge target than the artifact
- `MissingIdentifyingLabel`: metadata named a kind whose identifying label is absent (drift)
- `ExclusiveStateConflict`: several states of one exclusive dimension are present
- `MalformedMetadata`: the body's metadata block could not be parsed

## Queue evaluation and transition planning

The planner is the pure, deterministic state-machine layer over classified artifacts. It computes the read-side parts of the runtime guarantees below (authority, preconditions, postconditions) without loading fresh state or applying effects; a later executor phase does that against the `Forge` trait.

Queue matching: a classified artifact matches a queue when its kind equals the queue's artifact kind and every label the queue requires is present. Because exclusive state dimensions project to mutually exclusive labels, a `code + ready` queue naturally excludes `blocked` and `in-progress` code issues.

Transition planning checks, in order, and collects every problem:

- the transition is declared (else `UnknownTransition`)
- the role is authorized for the transition (else `Unauthorized`)
- the artifact's kind matches the transition's artifact kind (else `ArtifactKindMismatch`; the label/gate/state checks are skipped when the kind is wrong)
- each effect's label precondition holds: a `remove_label` target must be present (else `StalePrecondition`) and an `add_label` target must be absent (else `ContradictedPrecondition`)
- every required gate is satisfied — a gate is satisfied when some satisfying transition's added labels are all present (else `GateNotSatisfied`)
- applying the effects would not leave an exclusive dimension in several states (else `ImpossibleState`)

The impossible-state check is the plan-time complement to the planned static check on contradictory effects: even before static validation rejects such a transition, the planner refuses to plan one against a concrete artifact.

A successful plan's effects and postconditions follow the transition's declared effect order, so plans are deterministic and safe for snapshot-style assertions.

## Runtime guarantees

Every transition execution must:

1. load fresh Forge state for the target artifact
2. classify it according to the validated workflow
3. check role authority and transition preconditions
4. apply effects through an idempotent executor
5. verify postconditions or emit diagnostics

Agents must not mutate Forge state directly when operating under workflow control. Generated tools are the transition boundary.

## Effects

Workflow effects are the closed `plan::WorkflowEffect` enum so executors and reconcilers must handle every variant. Variants cover label add/remove, assignee set/remove, comment creation, issue and PR creation requests, lease update/release, and PR merge requests.

Only `AddLabel` and `RemoveLabel` are produced today, because current transition specs model only label effects. The remaining variants are explicit placeholders: they round out the set an executor must handle so later phases can add assignee, comment, create, lease, and merge effects without breaking exhaustive matches. The planning tests document this by asserting that label effects are the only ones a plan currently emits.

Create effects (`CreateIssue`, `CreatePullRequest`) carry correlation keys. Retrying a create effect with the same correlation key must return the existing artifact when it already exists; that idempotency is enforced by the executor phase, not the planner.

## Claims and leases

A claim is a lease, not permanent ownership. Lease metadata should include role, worker or run ID, claim time, heartbeat time, and expiration time.

Expired leases are handled by recovery policy. Common actions are requeue, extend, escalate, or mark for operator review.

## Reconciliation

The reconciler periodically scans Forge artifacts and command journals. It must detect:

- impossible label combinations
- expired leases
- partial transitions
- duplicated artifacts with the same correlation key
- missing required relations
- merged PRs whose linked code issue remains open
- validation failure labels on merged PRs

Repairs should be deterministic when safe. Ambiguous drift should be routed to an owner or operator queue with a diagnostic comment.

## Compilation outputs

`compile::compile` produces a `CompiledWorkflow` with:

- `RoleManifest` per role, embedding its `PromptManifest` and role-specific `tools`
- `ToolManifest` entries (intent-level, one per authorized transition)
- `QueueManifest` entries with subscribers, for runtime queue evaluation
- `LabelManifest` (a list of `LabelSpec` with `LabelUsage` annotations) for Forge label setup
- `TransitionManifest` entries forming the runtime transition table

Still planned: optional generated Rust code for statically checked workflows, and generated tool bodies that enforce preconditions and apply effects.

Generated tools expose intent-level operations such as `claim_code` or `record_test_failure`, not generic Forge mutation operations. Each `ToolManifest` is named after its transition and carries that transition's artifact, required gates, and effects.
