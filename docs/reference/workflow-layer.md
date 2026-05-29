# Workflow layer reference

This page defines the intended contract for the `harness-workflow` crate. Phases 2–13 have landed spec validation, artifact/metadata modeling, classification, first-class relation declarations, compilation to manifests, pure queue evaluation/activation and transition planning, external-signal gates, runtime execution of labels, assignees, comments, PR creates, and PR merges through the `Forge` trait, idempotent issue/PR creation, and recovery primitives (leases, command journaling, and reconciliation). See "Implementation status" for what exists today.

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

- `spec::RawWorkflowSpec` and its raw child structs (`RawRole`, `RawLabel`, `RawArtifactKind`, `RawRelation`, `RawStateDimension`, `RawState`, `RawQueue`, `RawTransition`, `RawEffect`, `RawGate`, `RawGateCondition`) load from serde input.
- `validated::ValidatedWorkflow` is the normalized model. It has no public constructor; the only way to build one is `RawWorkflowSpec::validate` / `validate::validate`, so compiler and runtime APIs added later can require it by type.
- `ids` provides typed ids: `RoleId`, `LabelId`, `ArtifactKindId`, `StateDimensionId`, `StateId`, `QueueId`, `TransitionId`, `GateId`.
- `diagnostics` provides `Diagnostic`, `Severity`, `SymbolKind`, `ReferenceSite`, and the `ValidationErrors` collection.

Phase 3 added artifact/Forge mapping, metadata blocks, and classification:

- `artifact::ArtifactTarget` maps each artifact kind to a Forge `Issue` or `PullRequest`. Artifact kinds now carry `target` and `identifying_labels`.
- State dimensions carry an `exclusive` flag (default `true`) so the classifier can reject impossible label combinations.
- `metadata::WorkflowMetadata` (with `Lease`) is the machine-readable block parsed from and rendered into artifact bodies. See "Metadata block format".
- `classify::Classifier` turns a `harness_forge::Issue` or `PullRequest` into a typed `ClassifiedArtifact`, or a `ClassificationError` carrying `ClassificationDiagnostic`s. See "Artifact classification".
- `harness-workflow` now depends on `harness-forge` because classification consumes Forge domain types.

Not yet modeled: `invariant` and spec-level `recovery_policy`. Effects cover label add/remove plus assignee, comment, pull-request create, and pull-request merge requests.

Gate/transition wiring is modeled in both directions for role actions: a transition lists `requires_gates`, and a gate lists `satisfied_by` transitions. A gate may also declare a portable external condition over artifact labels/state.

Phase 4 added compilation:

- `compile::compile` (also `ValidatedWorkflow::compile`) projects a validated workflow into a `CompiledWorkflow`. Compilation is infallible because it consumes an already-validated workflow.
- `RoleManifest` carries role id, charter, concurrency hint, subscribed queues, transition `authority`, role-specific `tools`, and a `PromptManifest`.
- `ToolManifest` is an intent-level operation (named after its transition) carrying artifact, required gates, and effects. Tools are derived from the transitions a role is authorized for, so a role can never see a tool outside its authority.
- `QueueManifest` adds `subscribers` and activation policy to each queue; `TransitionManifest` is the runtime transition table; `LabelManifest`/`LabelSpec`/`LabelUsage` enumerate every label a workflow site needs and why.
- `PromptManifest`/`PromptSection` hold deterministic prompt sections (`Role`, `Charter`, `Queues`, `Authorized actions`) with a stable `render` method.
- Roles now carry an optional `concurrency` hint (`RawRole`/`ValidatedRole`), compiled into the role manifest.

Not yet implemented from compilation: generated tool bodies and optional generated Rust code.

Phase 5 added pure queue evaluation and transition planning:

- `plan::Planner` (also `ValidatedWorkflow::planner`) borrows a `ValidatedWorkflow` and never touches a Forge backend. It matches classified artifacts against queues, evaluates queue activation, and plans transitions into typed effects.
- `plan::matches_queue` matches a `ClassifiedArtifact` against any `QueueQuery`; `plan::queue_active` decides whether matched members should be serviced now. `QueueQuery` is implemented by both `ValidatedQueue` and the compiled `QueueManifest`, so the same predicates serve the validated model and a compiled runtime table.
- `plan::WorkflowEffect` is the closed planning-effect enum. `plan::Postcondition` carries the label and assignee conditions that must hold after a plan applies. `plan::TransitionPlan` bundles transition, role, artifact kind, target, effects, and postconditions.
- `plan::PlanError` collects `plan::PlanDiagnostic`s (unauthorized role, artifact-kind mismatch, stale/contradicted label preconditions, unsatisfied transition/external gates, and impossible resulting states).

Phase 6 added runtime execution of transitions through the `Forge` trait:

- `execute::Executor` (also `ValidatedWorkflow::executor`) is generic over `F: Forge + ?Sized`, so it runs against a concrete backend or a `&dyn Forge`. It owns no durable state, so one executor is reusable across executions.
- `Executor::execute` runs the full transition loop against fresh state: load the target by item number, classify it, re-plan the transition (re-checking authority, preconditions, gates, and resulting states), apply supported effects, and verify the postconditions. It applies labels and assignees in one backend update and posts idempotent comments before that update. It returns an `ExecutionReport` or a typed `ExecutionError`.
- `ExecutionError` separates the failure classes a runtime must distinguish: `Validation` (undeclared transition, unauthorized role, artifact-kind mismatch), `Precondition` (stale/contradicted labels, unsatisfied gate, impossible state), `Backend` (any `ForgeError`), plus `Classification`, `TargetMissing`, `UnsupportedEffect`, `UnresolvedAssignee`, `MissingCorrelationKey`, `UnresolvedPullRequestCreate`, and `PostconditionFailed`.
- `Executor::ensure_issue` and `Executor::ensure_pull_request` are idempotent create helpers: they search existing artifacts for a metadata `correlation_key`, return the existing artifact if found, or stamp the key into the new metadata block and create it. `EnsureOutcome` reports whether the artifact was `Created` or `Existing`.

Phase 7 added recovery primitives — leases, command journaling, and reconciliation:

- `lease::LeasePlanner` is the pure decision layer over a `LeasePolicy` (a heartbeat time-to-live). `acquire`, `heartbeat`, and `release` compute the next `metadata::Lease` (or a `LeaseConflict`) from the current lease, a worker identity, and `now`. It never touches a backend. `lease::LeaseManager` is generic over `F: Forge + ?Sized`; it applies those decisions by rewriting the target artifact's metadata block through a single body update, loading fresh state first.
- `journal::CommandJournal` is an async trait so durable storage can be added later. A `CommandRecord` carries a caller-chosen `CommandId`, the target, the transition/role, the planned effects, a `CommandState`, a detail, and timestamps. `CommandState` is `Planned`, `Applying`, `Completed`, `Failed`, or `Reconciled`; `Planned`/`Applying` are *incomplete*, the rest terminal. `journal::InMemoryJournal` is a shared-store implementation whose clones share one append-ordered log, so a test can simulate a restart by attaching a fresh handle.
- `execute::Executor::execute_journaled` records the lifecycle (`Planned` → `Applying` → `Completed`/`Failed`) around the existing execute loop; `Executor::plan` previews a transition's plan without mutating. Planning failures are returned unjournaled because no mutation was attempted.
- `reconcile::Reconciler` scans `ArtifactSnapshot`s and `CommandRecord`s and decides repair/escalation actions through a `RecoveryPolicy`. See "Command journal", "Claims and leases", and "Reconciliation".

Phase 9c added merge execution and post-merge projection:

- `Executor::execute` applies `MergePullRequest` through the `Forge` merge API. The merge runs *before* the label/assignee commit point and is skipped when the freshly loaded pull request is already merged, so a merge is applied at most once even when a crash lands the merge but loses the response.
- The post-merge `landed` and `owner-pending` labels are modeled as ordinary `add_label` effects on the merge transition (preferring modeling over executor special-casing), so they are projected by the same atomic label update and survive on the now-closed pull request. The merge transition's `merge-ready`/`landed`/`owner-pending` labels also double as the "already done" marker that makes a retry's planner refuse to re-run a completed merge.

Phase 10 added pull-request idempotent create:

- `Executor::ensure_pull_request` mirrors `ensure_issue`: it searches pull requests for a metadata `correlation_key`, stamps that key into new PR bodies, and returns `EnsureOutcome::Existing` on retry.
- `Executor::execute` applies `CreatePullRequest` through `ensure_pull_request` when the effect has a correlation key and `ExecutionContext` supplies the concrete `CreatePullRequest` input for that transition.

Phase 11 added external-signal gates:

- `RawGate`/`ValidatedGate` may declare one `condition`: `label_present` or `state_equals`. The condition is satisfied from the classified artifact's current Forge-projected labels/state.
- Transition-satisfied gates continue to use `satisfied_by`; a required gate passes when either its external condition holds or one of its satisfying transition outcomes is visible.

Phase 12a added first-class relations:

- `RawRelation`/`ValidatedRelation` declare `parent`, `dependency`, and `produced_pr` links between source and target artifact kinds.
- Validation checks relation endpoints against declared artifact kinds.
- Classification surfaces typed relations from metadata `parents`/`dependencies` item numbers using the validated declarations.

Phase 13 added queue activation policy:

- `RawQueue`/`ValidatedQueue` carry optional `min_depth` and `max_age` fields; raw `max_age` is seconds.
- `plan::queue_active(queue, members, now)` is pure and activates a non-empty queue when it has no policy, reaches `min_depth`, or its oldest timestamped member reaches `max_age`.
- The executor, leases, journal, and reconciler are unchanged.

Not yet implemented: relation-driven `dependency_gate`, multi-kind/disjunctive queue matching, lease effects inside `Executor::execute`, expressing lease effects in transition specs, applying reconciler actions automatically, and durable journal/lease storage backends.

Phase 8 added robustness and crash-injection tests (no new runtime types):

- `tests/support/crash.rs` provides `CrashForge`, a `Forge` wrapper that injects a deterministic fault before or after a chosen operation's chosen call, so a backend mutation can fail either before it lands (state intact) or after it lands (state changed, caller sees failure).
- `tests/crash_injection.rs` proves crash-before/after retry safety, a fault matrix showing label effects are applied at most once, journaled restart recovery (partial transition → repair, landed effect → reconciled), and at-most-once claiming under duplicated tool calls and interleaved workers.
- `tests/safety_properties.rs` proves the safety assertions registered in `robustness-guarantees.md`: no duplicate issue/PR create per correlation key under crash, no two active leases per exclusive claim, no merge before required gates pass (review/testing in the five-role fixture and external CI/review/testing in an inline three-gate workflow), a gated merge executes at most once and projects the post-merge `landed`/`owner-pending` labels, failed review gate returns work to the engineer, expired in-progress work becomes visible for recovery, and impossible label combinations are detected by both the executor and the reconciler.

See `robustness-guarantees.md` for the full safety-property register and the limitations these tests surfaced (notably that lease acquisition is not yet a compare-and-swap).

## Spec primitives

A workflow spec contains these logical primitives.

| Primitive | Meaning |
| --- | --- |
| `role` | Actor authority, queues, concurrency limits, and prose charter |
| `artifact_kind` | Logical item with a Forge `target` (issue or PR) and `identifying_labels` |
| `state_dimension` | Named state group with an `exclusive` flag, projected as labels |
| `queue` | Query that selects artifacts needing attention, with optional read-side activation policy |
| `transition` | Guarded action authorized for one or more roles; its effects may update labels, set/remove role-resolved assignees, create comments, request pull-request creation, or request PR merge |
| `gate` | Condition that unlocks another transition, either from sibling transition outcomes or a Forge-projected label/state condition |
| `relation` | Typed link between artifacts, such as parent, dependency, or produced PR |
| `invariant` | Condition that must hold during runtime scans |
| `recovery_policy` | What to do with expired leases, partial transitions, and drift |

Labels are a portable Forge projection of workflow state. A `relation` declares `{ kind, source, target }`, where `kind` is `parent`, `dependency`, or `produced_pr` and endpoints are artifact kinds. Non-label effect payloads stay portable: assignee effects reference declared role ids (the runtime resolves a role to a concrete worker/user), comments carry a prose/template `body`, `create_pull_request` carries only an optional `correlation_key` while branch, title, body, labels, and assignees come from runtime context, and `merge_pull_request` has no payload. The workflow layer may use metadata blocks in bodies or comments for information that is not represented by the current Forge interface, such as correlation keys and relation item numbers.

The `reference-delivery.json` fixture transcribes the reference delivery design (`docs/explanation/reference-workflow.md`) into these primitives; its label-state-machine core plus non-label effects, external-signal gates, relation declarations, and queue activation policy validate, compile, and plan (`tests/reference_delivery.rs`). The remaining capabilities that design still needs — relation-driven `dependency_gate` and multi-kind/disjunctive queue matching — are the prioritized backlog in `docs/explanation/reference-workflow-gaps.md`.

## Static validation

Validation must reject or diagnose:

- duplicate role, label, artifact, state-dimension, queue, transition, or gate IDs (implemented; state ids are checked for uniqueness within each dimension)
- references to undeclared roles, labels, artifact kinds, queues, transitions, gates, relation endpoints, or gate-condition labels/states (implemented)
- transitions whose effects contradict declared mutually exclusive dimensions (planned)
- gates that cannot be satisfied by any declared transition or external condition (planned)
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

`render_metadata_block` and `parse_metadata_block` are inverses. Parsing returns `Ok(None)` when no block is present, `Ok(Some(_))` when one parses, and `Err(MetadataError)` when a block is present but unterminated or contains invalid JSON. Relation projections (`parents`, `dependencies`) are Forge item numbers in the same repository, matching the shared issue/PR number namespace; classifiers type them by consulting declared `relation` primitives.

## Artifact classification

`Classifier::classify_issue` and `Classifier::classify_pull_request` interpret a Forge artifact under a `ValidatedWorkflow`. Classification reads labels and the metadata block; it never mutates Forge state.

Kind resolution: when metadata names a `kind`, that kind is authoritative. Otherwise the kind is inferred from `identifying_labels` — a kind matches when all of its identifying labels are present, and the most specific match (most identifying labels) wins.

State resolution: for each dimension, the active states are those whose label is present. An exclusive dimension with more than one active state is an impossible combination.

Relation resolution: metadata `parents` and `dependencies` contain Forge item numbers. The classifier emits `ClassifiedRelation`s for declarations whose source is the artifact kind, preserving the linked item number and the declared possible target artifact kinds. `produced_pr` declarations are read from the `parents` projection on PR artifacts.

Success yields a `ClassifiedArtifact` (kind, target, source, optional `updated_at`, per-dimension states, parsed metadata, typed relations, raw labels). Otherwise a `ClassificationError` collects every `ClassificationDiagnostic`:

- `Unclassified`: no kind matched and metadata named none
- `AmbiguousArtifactKind`: several kinds matched equally well
- `UnknownMetadataKind`: metadata named an undeclared kind
- `TargetMismatch`: the kind maps to a different Forge target than the artifact
- `MissingIdentifyingLabel`: metadata named a kind whose identifying label is absent (drift)
- `ExclusiveStateConflict`: several states of one exclusive dimension are present
- `MalformedMetadata`: the body's metadata block could not be parsed

## Queue evaluation and transition planning

The planner is the pure, deterministic state-machine layer over classified artifacts. It computes the read-side parts of the runtime guarantees below (authority, preconditions, postconditions) without loading fresh state or applying effects; a later executor phase does that against the `Forge` trait.

Queue matching: a classified artifact matches a queue when its kind equals the queue's artifact kind and every label the queue requires is present. Because exclusive state dimensions project to mutually exclusive labels, a `code + ready` queue naturally excludes `blocked` and `in-progress` code issues. Matching does not consider activation policy.

Queue activation: a queue with no activation policy is active whenever it has at least one matched member. A queue with `min_depth` and/or `max_age` is active when it is non-empty and either its member count is at least `min_depth` or the oldest timestamped member is at least `max_age` old at `now`. `max_age` uses the classified artifact's Forge `updated_at` timestamp; snapshot-classified artifacts without timestamps cannot satisfy the age branch.

Transition planning checks, in order, and collects every problem:

- the transition is declared (else `UnknownTransition`)
- the role is authorized for the transition (else `Unauthorized`)
- the artifact's kind matches the transition's artifact kind (else `ArtifactKindMismatch`; the label/gate/state checks are skipped when the kind is wrong)
- each label effect's precondition holds: a `remove_label` target must be present (else `StalePrecondition`) and an `add_label` target must be absent (else `ContradictedPrecondition`); non-label effects have no label precondition
- every required gate is satisfied — a gate is satisfied when its external label/state condition holds or some satisfying transition's added labels are all present (else `GateNotSatisfied`)
- applying the effects would not leave an exclusive dimension in several states (else `ImpossibleState`)

The impossible-state check is the plan-time complement to the planned static check on contradictory effects: even before static validation rejects such a transition, the planner refuses to plan one against a concrete artifact.

A successful plan's effects follow the transition's declared effect order. Postconditions are derived from label and assignee effects and keep that relative order, so plans are deterministic and safe for snapshot-style assertions. Comment effects do not produce postconditions because comments are append-only events rather than label-style state predicates.

## Runtime guarantees

Every transition execution must:

1. load fresh Forge state for the target artifact
2. classify it according to the validated workflow
3. check role authority and transition preconditions
4. apply effects through an idempotent executor
5. verify postconditions or emit diagnostics

`Executor::execute` implements this loop today for labels, assignees, comments, pull-request creates, and pull-request merges. It never trusts a plan computed against stale state: it re-loads and re-plans against fresh state immediately before mutating, and it refuses to mutate at all if planning fails, if a plan contains an unsupported effect, if an assignee role has no runtime user binding, if a create lacks a correlation key, or if no pull-request create input is bound. It posts idempotent comments first, ensures pull requests next, merges the target pull request next (skipping an already-merged target), then applies label and assignee changes together in one backend update. Because creates and merges precede the label commit point, retries can dedupe a landed create and finish post-create state, while post-merge labels become the marker that makes a retry refuse to re-run; already-merged targets are skipped, so the merge is at most once. Postconditions are verified by re-reading the artifact's labels and assignees after the update; a mismatch yields `PostconditionFailed`.

Idempotency: re-running a label transition that already applied fails as a `Precondition` error (the source label is gone and/or the target label is present), so a retry never double-applies. `SetAssignee` is cleanly idempotent when the resolved user is already assigned, and `RemoveAssignee` is cleanly idempotent when the resolved user is already absent. `CreateComment` is guarded by a hidden marker appended to the body (`<!-- harness:comment-key=<transition>:<comment-index> -->`); the executor lists comments on the same target and skips posting when the marker already exists, so a retry after a crash-before-state-flip cannot duplicate the comment. Comments have no postcondition; instead the marker check is the verified idempotency mechanism. Idempotent artifact create is handled by `Executor::ensure_issue` and `Executor::ensure_pull_request` through correlation keys.

Agents must not mutate Forge state directly when operating under workflow control. Generated tools are the transition boundary.

## Effects

Workflow effects are the closed `plan::WorkflowEffect` enum so executors and reconcilers must handle every variant. Variants cover label add/remove, assignee set/remove, comment creation, issue and PR creation requests, lease update/release, and PR merge requests.

Transition specs now emit `AddLabel`, `RemoveLabel`, `SetAssignee`, `RemoveAssignee`, `CreateComment`, `CreatePullRequest`, and `MergePullRequest`. `SetAssignee`/`RemoveAssignee` carry a workflow role id, not a Forge user id; `execute::ExecutionContext` resolves the role to a concrete Forge user at runtime, and missing bindings fail with `ExecutionError::UnresolvedAssignee` before mutation. `CreateComment` carries a prose/template `body`. `CreatePullRequest` carries an optional correlation key only; branch, title, body, labels, and assignees come from `ExecutionContext`, and a missing key or input fails before mutation. `MergePullRequest` has no payload.

Leases are not yet emitted as transition effects (`UpdateLease`/`ReleaseLease` remain placeholders). Lease changes go through `lease::LeaseManager` as standalone operations on the metadata block; see "Claims and leases".

`Executor::execute` applies `AddLabel`, `RemoveLabel`, `SetAssignee`, `RemoveAssignee`, `CreateComment`, `CreatePullRequest`, and `MergePullRequest` through the `Forge` trait. `CreatePullRequest` runs before the label/assignee commit point through `Executor::ensure_pull_request`, using the effect's correlation key and the transition-bound runtime input. `MergePullRequest` merges through the Forge merge API at most once (an already-merged target is skipped) using a default merge-commit method; its transition's post-merge labels are projected as ordinary `add_label` effects in the same atomic update. The executor still rejects `UpdateLease` and `ReleaseLease` with `ExecutionError::UnsupportedEffect` before any mutation.

Create effects use correlation keys for idempotent retries: `CreateIssue` requires one, while `CreatePullRequest` must have one to execute. `Executor::ensure_issue` and `Executor::ensure_pull_request` stamp the key into artifact metadata and search existing artifacts before creating.

## Command journal

The command journal records the lifecycle of each runtime command so a crash between deciding to mutate and finishing the mutation is recoverable. `CommandJournal` is async with these operations: `append` (idempotent on `CommandId` — a repeated id is a no-op), `transition_state` (move to a new `CommandState` with detail and timestamp; `NotFound` for unknown ids), `get`, `list` (append order), and a defaulted `incomplete` that returns the `Planned`/`Applying` records a reconciler must investigate.

`CommandRecord::planned` constructs the initial record. `execute::Executor::execute_journaled` records `Planned` (with the previewed effects) before any mutation, advances to `Applying` immediately before applying, and finishes at `Completed` or `Failed`. If the process stops between `Applying` and the terminal update, the entry stays incomplete and the reconciler can repair it after restart.

## Claims and leases

A claim is a lease, not permanent ownership. `metadata::Lease` records role, worker or run ID, claim time, heartbeat time, and expiration time; `Lease::is_expired(now)` is true once `now >= expires_at`.

`LeasePlanner` enforces the rules: `acquire` grants when there is no lease or the existing one has expired (reclaiming the expired holder), refreshes in place when the same worker already holds an unexpired lease (preserving `claimed_at`), and fails with `LeaseConflict::HeldByOther` when a different worker holds a live lease. `heartbeat` extends the holder's lease to `now + ttl` (failing `NotHeld`/`HeldByOther` otherwise). `release` is idempotent for the holder and for an already-empty lease, and fails `HeldByOther` for a peer — forcibly clearing another worker's lease is the reconciler's job, not a peer's. `LeaseManager` applies these decisions against a `Forge` by rewriting the metadata block in a single body update.

Expired leases are handled by recovery policy. Common actions are requeue, extend, escalate, or mark for operator review.

## Reconciliation

The reconciler scans Forge artifacts and the command journal and decides what to repair or escalate; applying the decision is left to the executor and lease manager. `Reconciler::scan` is pure and deterministic: given `ArtifactSnapshot`s, `CommandRecord`s, and `now`, it returns a `ReconcileReport` whose parallel `findings` and `actions` follow a stable order (each snapshot's expired lease then its classification problems, in snapshot order, then incomplete journal commands in journal order). `Reconciler::reconcile` is the async convenience that loads snapshots and journal entries from a `Forge` and a `CommandJournal`, then calls `scan`.

Findings (`ReconcileFinding`) cover: `ExpiredLease`, `ImpossibleState` (an exclusive dimension with several active states), `ClassificationDrift` (other classification failures), `PartialTransition` (a journaled command whose label effects are not all realized), and `StaleCommand` (an incomplete command whose effects already landed or whose target is gone). Each finding gets exactly one `RecoveryAction`: `RequeueLease`, `Escalate`, `Repair { effects }`, `MarkReconciled`, or `Diagnose`.

`RecoveryPolicy` is the hook layer: one defaulted method per finding class, so a workflow overrides only what it needs. `DefaultRecoveryPolicy` requeues expired leases, escalates impossible states and drift, repairs partial transitions with their pending effects, and marks stale commands reconciled. Still planned for the reconciler: duplicated correlation keys, missing required relations, merged PRs whose linked code issue stays open, and validation-failure labels on merged PRs.

## Compilation outputs

`compile::compile` produces a `CompiledWorkflow` with:

- `RoleManifest` per role, embedding its `PromptManifest` and role-specific `tools`
- `ToolManifest` entries (intent-level, one per authorized transition)
- `QueueManifest` entries with subscribers and activation policy, for runtime queue evaluation
- `LabelManifest` (a list of `LabelSpec` with `LabelUsage` annotations) for Forge label setup
- `TransitionManifest` entries forming the runtime transition table

Still planned: optional generated Rust code for statically checked workflows, and generated tool bodies that enforce preconditions and apply effects.

Generated tools expose intent-level operations such as `claim_code` or `record_test_failure`, not generic Forge mutation operations. Each `ToolManifest` is named after its transition and carries that transition's artifact, required gates, and effects.
