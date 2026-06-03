# Workflow layer reference

This page defines the intended contract for the `temper-workflow` crate. Phases 2–14 plus native Forge-state phases A/B/C have landed spec validation, artifact/metadata modeling, classification, first-class relation declarations, native dependency-link classification with metadata fallback, compilation to manifests, pure queue evaluation/activation and transition planning, external/runtime-signal gates (including native CI and reviews), runtime execution of labels, assignees, comments, PR creates, reviewer requests, review submissions, and PR merges through the `Forge` trait, idempotent issue/PR creation, and recovery primitives (leases, command journaling, and reconciliation). See "Implementation status" for what exists today.

## Scope

`temper-workflow` owns workflow policy and orchestration on top of `temper-forge`. It must not contain concrete Forge backend code or agent-provider code.

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

- `spec::RawWorkflowSpec` and its raw child structs (`RawRole`, `RawRolePrompt`, `RawExternalTool`, `RawLabel`, `RawArtifactKind`, `RawRelation`, `RawStateDimension`, `RawState`, `RawQueue`, `RawTransition`, `RawEffect`, `RawGate`, `RawGateCondition`) load from serde input.
- `validated::ValidatedWorkflow` is the normalized model. It has no public constructor; the only way to build one is `RawWorkflowSpec::validate` / `validate::validate`, so compiler and runtime APIs added later can require it by type.
- `ids` provides typed ids: `RoleId`, `ExternalToolId`, `LabelId`, `ArtifactKindId`, `StateDimensionId`, `StateId`, `QueueId`, `TransitionId`, `GateId`.
- `diagnostics` provides `Diagnostic`, `Severity`, `SymbolKind`, `ReferenceSite`, and the `ValidationErrors` collection.

Phase 3 added artifact/Forge mapping, metadata blocks, and classification:

- `artifact::ArtifactTarget` maps each artifact kind to a Forge `Issue` or `PullRequest`; `artifact::ArtifactRef` names a linked issue or pull request as a repository-qualified reference, with same-repository shorthand for compatibility. Artifact kinds now carry `target` and `identifying_labels`.
- State dimensions carry an `exclusive` flag (default `true`) so the classifier can reject impossible label combinations. Individual states may also list the artifact kinds they are legal for; an empty list means all kinds.
- `metadata::WorkflowMetadata` (with `Lease`) is the machine-readable block parsed from and rendered into artifact bodies. See "Metadata block format".
- `classify::Classifier` turns a `temper_forge::Issue` or `PullRequest` into a typed `ClassifiedArtifact`, or a `ClassificationError` carrying `ClassificationDiagnostic`s. See "Artifact classification".
- `temper-workflow` now depends on `temper-forge` because classification consumes Forge domain types.

Not yet modeled: `invariant` and spec-level `recovery_policy`. Effects cover label add/remove plus assignee, comment, pull-request create, reviewer request, review submission, and pull-request merge requests.

Gate/transition wiring is modeled in both directions for role actions: a transition lists `requires_gates`, and a gate lists `satisfied_by` transitions. A gate may also declare a portable condition over artifact labels/state or runtime-supplied signals such as dependency resolution, native CI, and native reviews.

Phase 4 added compilation:

- `compile::compile` (also `ValidatedWorkflow::compile`) projects a validated workflow into a `CompiledWorkflow`. Compilation is infallible because it consumes an already-validated workflow.
- `RoleManifest` carries role id, legacy charter text, structured prompt extension, declared external-tool metadata, concurrency hint, subscribed queues, transition `authority`, role-specific workflow `tools`, and a `PromptManifest`.
- `ToolManifest` is an intent-level workflow operation (named after its transition) carrying artifact, required gates, and effects. Tools are derived from the transitions a role is authorized for, so a role can never see a workflow tool outside its authority. `ExternalToolManifest` carries user-declared non-workflow tool metadata only; it is not executable unless the runner binds a provider.
- `QueueManifest` adds `subscribers`, multi-kind/disjunctive matching, and activation policy to each queue; `TransitionManifest` is the runtime transition table; `LabelManifest`/`LabelSpec`/`LabelUsage` enumerate every label a workflow site needs and why.
- `PromptManifest`/`PromptSection` hold deterministic prompt sections (`Role and workflow`, `Work item context`, `Subscribed queues`, `Authorized workflow actions`, `User-declared external tools`, `User guidance`, and optional `User tool guidance`) with a stable `render` method. Generated sections describe workflow mechanics and authority boundaries only; user-authored behavior comes from `charter` and `prompt`.
- Roles now carry an optional `concurrency` hint (`RawRole`/`ValidatedRole`), a structured prompt extension (`prompt.guidance` plus `prompt.tool_guidance`), and `external_tools` declarations (`id`, `description`, `required`, `constraints`, and `guidance`), compiled into the role manifest.

Not yet implemented from compilation: generated tool bodies and optional generated Rust code.

Phase 5 added pure queue evaluation and transition planning:

- `plan::Planner` (also `ValidatedWorkflow::planner`) borrows a `ValidatedWorkflow` and never touches a Forge backend. It matches classified artifacts against queues, evaluates queue activation, and plans transitions into typed effects.
- `plan::matches_queue` matches a `ClassifiedArtifact` against any `QueueQuery`; `plan::queue_active` decides whether matched members should be serviced now. `QueueQuery` is implemented by both `ValidatedQueue` and the compiled `QueueManifest`, so the same predicates serve the validated model and a compiled runtime table.
- `plan::WorkflowEffect` is the closed planning-effect enum. `plan::Postcondition` carries the label and assignee conditions that must hold after a plan applies. `plan::TransitionPlan` bundles transition, role, artifact kind, target, effects, and postconditions.
- `plan::PlanError` collects `plan::PlanDiagnostic`s (unauthorized role, artifact-kind mismatch, stale/contradicted label preconditions, unsatisfied transition/external gates, and impossible resulting states).

Phase 6 added runtime execution of transitions through the `Forge` trait:

- `execute::Executor` (also `ValidatedWorkflow::executor`) is generic over `F: Forge + ?Sized`, so it runs against a concrete backend or a `&dyn Forge`. It owns no durable state, so one executor is reusable across executions.
- `Executor::execute` runs the full transition loop against fresh state: load the target by item number, classify it, re-plan the transition (re-checking authority, preconditions, gates, and resulting states), apply supported effects, and verify the postconditions. It applies labels and assignees in one backend update and posts idempotent comments before that update. It returns an `ExecutionReport` or a typed `ExecutionError`.
- `Executor::read_gate_signals` exposes the read-only load/classify/signal path for runner scans that need the same `GateSignals` used by execution and planning.
- `ExecutionError` separates the failure classes a runtime must distinguish: `Validation` (undeclared transition, unauthorized role, artifact-kind mismatch), `Precondition` (stale/contradicted labels, unsatisfied gate, impossible state), `Backend` (any `ForgeError`), plus `Classification`, `TargetMissing`, `UnsupportedEffect`, `UnresolvedAssignee`, `MissingCorrelationKey`, `UnresolvedPullRequestCreate`, and `PostconditionFailed`.
- `Executor::ensure_issue` and `Executor::ensure_pull_request` are idempotent create helpers: they search existing artifacts for a metadata `correlation_key`, return the existing artifact if found, or stamp the key into the new metadata block and create it. `EnsureOutcome` reports whether the artifact was `Created` or `Existing`.

Phase 7 added recovery primitives — leases, command journaling, and reconciliation:

- `lease::LeasePlanner` is the pure decision layer over a `LeasePolicy` (a heartbeat time-to-live). `acquire`, `heartbeat`, and `release` compute the next `metadata::Lease` (or a `LeaseConflict`) from the current lease, a worker identity, and `now`. It never touches a backend. `lease::LeaseManager` is generic over `F: Forge + ?Sized`; it applies those decisions by rewriting the target artifact's metadata block through a single body update, loading fresh state first.
- `journal::CommandJournal` is an async trait so durable storage can be added later. A `CommandRecord` carries a caller-chosen `CommandId`, the target, the transition/role, the planned effects, a `CommandState`, a detail, and timestamps. `CommandState` is `Planned`, `Applying`, `Completed`, `Failed`, or `Reconciled`; `Planned`/`Applying` are *incomplete*, the rest terminal. `journal::InMemoryJournal` is a shared-store implementation whose clones share one append-ordered log, so a test can simulate a restart by attaching a fresh handle.
- `execute::Executor::execute_journaled` records the lifecycle (`Planned` → `Applying` → `Completed`/`Failed`) around the existing execute loop; `Executor::plan` previews a transition's plan without mutating. Planning failures are returned unjournaled because no mutation was attempted.
- `reconcile::Reconciler` scans `ArtifactSnapshot`s and `CommandRecord`s and decides repair/escalation actions through a `RecoveryPolicy`. See "Command journal", "Claims and leases", and "Reconciliation".

Phase 9c added merge execution and post-merge projection:

- `Executor::execute` applies `MergePullRequest` through the `Forge` merge API. The merge runs *before* the label/assignee commit point and is skipped when the freshly loaded pull request is already merged, so a merge is applied at most once even when a crash lands the merge but loses the response.
- The post-merge `landed` and `alignment` labels are modeled as ordinary `add_label` effects on the merge transition (preferring modeling over executor special-casing), so they are projected by the same atomic label update and survive on the now-closed pull request. `landed`/`alignment` now double as the "already done" marker that makes a retry's planner refuse to re-run a completed merge; merge eligibility itself is derived from gates, not a stored `merge-ready` label.

Phase 10 added pull-request idempotent create:

- `Executor::ensure_pull_request` mirrors `ensure_issue`: it searches pull requests for a metadata `correlation_key`, stamps that key into new PR bodies, and returns `EnsureOutcome::Existing` on retry.
- `Executor::execute` applies `CreatePullRequest` through `ensure_pull_request` when the effect has a correlation key and `ExecutionContext` supplies the concrete `CreatePullRequest` input for that transition.

Phase 11 added external-signal gates, later extended for native reviews and CI failure routing:

- `RawGate`/`ValidatedGate` may declare one `condition`: `label_present`, `state_equals`, `dependencies_resolved`, `ci_passed`, `ci_failed`, `review_approved`, or `review_changes_requested`. `label_present`/`state_equals` are satisfied from classified labels/state; the others read runtime-supplied signals.
- `RawQueue`/`ValidatedQueue` may also carry a condition so queues can key off native signals such as `review_changes_requested` or `ci_failed`.
- Transition-satisfied gates continue to use `satisfied_by`; a required gate passes when either its condition holds or one of its satisfying transition outcomes is visible.

Phase 12a added first-class relations:

- `RawRelation`/`ValidatedRelation` declare `parent`, `dependency`, and `produced_pr` links between source and target artifact kinds.
- Validation checks relation endpoints against declared artifact kinds.
- Classification surfaces typed relations from metadata `parents` and native or fallback `dependencies` item numbers using the validated declarations.

Phase 12b plus native dependency links added the relation-driven `dependency_gate` and a mechanical reconcile unblock:

- The `dependencies_resolved` gate condition (`GateCondition::DependenciesResolved`) is satisfied when every `dependency` relation target of the artifact has landed. It is vacuously true for an artifact with no dependency relations.
- "Has it landed" is a runtime signal, never derived in the pure planner. Runtime layers derive `plan::DependencyStatus` from fresh Forge state in each target's repository: issue targets count as landed when closed, pull-request targets when merged. If a target repository cannot be read on a scan, that target is treated as not landed and recorded as a dependency read failure rather than satisfying the gate.
- `Planner::dependency_unblocks(artifact, deps)` returns the mechanical (actor-less) `MechanicalPlan`s an artifact admits: transitions gated on a `dependencies_resolved` gate whose preconditions, gates, and resulting states all hold. It requires the artifact to declare at least one `dependency` relation, so a blocked artifact with no recorded dependency is never auto-unblocked.
- The reconciler derives dependency status during `reconcile`, then turns each available unblock into a `DependenciesResolved` finding and an `Unblock { effects }` action (see "Reconciliation"). A dependency-gated blocked artifact with zero dependency relations instead produces `BlockedWithoutDependencies` plus a `Diagnose` action; this is observability only and does not weaken the gate.

Native reviews (ADR 0016) added `RequestReviewers` and `SubmitReview` effects. The executor resolves reviewer roles through `ExecutionContext`, asks the Forge to request those reviewers, submits native review decisions for workflow review transitions, and derives `ReviewStatus` from fresh requested reviewers plus native review events before planning gates.

Phases 13–14 added queue primitive extensions:

- `RawQueue`/`ValidatedQueue` carry optional `min_depth` and `max_age` fields; raw `max_age` is seconds.
- A queue selects one or more artifact kinds. Raw specs use the `artifact` field as either a string or a list of strings; the validated and compiled models normalize this to `artifacts`.
- Queue `labels` are common AND labels. Optional `any_of` entries are OR branches, each with its own AND `labels` list.
- `plan::queue_active(queue, members, now)` is pure and activates a non-empty queue when it has no policy, reaches `min_depth`, or its oldest timestamped member reaches `max_age`.
- The executor, leases, journal, and reconciler are unchanged.

Reconciler actions are now applied automatically through `recover::Applier` (including the mechanical `Unblock`); see "Applying reconciler actions". Not yet implemented: lease effects inside `Executor::execute`, expressing lease effects in transition specs, projecting `Escalate`/`Diagnose` into labels or comments, and durable journal/lease storage backends.

Phase 8 added robustness and crash-injection tests (no new runtime types):

- `tests/support/crash.rs` provides `CrashForge`, a `Forge` wrapper that injects a deterministic fault before or after a chosen operation's chosen call, so a backend mutation can fail either before it lands (state intact) or after it lands (state changed, caller sees failure).
- `tests/crash_injection.rs` proves crash-before/after retry safety, a fault matrix showing label effects are applied at most once, journaled restart recovery (partial transition → repair, landed effect → reconciled), and at-most-once claiming under duplicated tool calls and interleaved workers.
- `tests/safety_properties.rs` proves the safety assertions registered in `robustness-guarantees.md`: no duplicate issue/PR create per correlation key under crash, no two active leases per exclusive claim, no merge before required native review and CI gates pass, a gated merge executes at most once and projects the post-merge `landed`/`alignment` labels, failed review/CI gates return work to the engineer, expired in-progress work becomes visible for recovery, and impossible label combinations are detected by both the executor and the reconciler.

See `robustness-guarantees.md` for the full safety-property register and the limitations these tests surfaced. Lease acquisition is now a compare-and-swap: `LeaseManager` captures each artifact's `Version` at load time and writes the lease conditionally (ADR 0013), so two acquirers over the same "no lease" snapshot cannot both win.

## Spec primitives

A workflow spec contains these logical primitives.

| Primitive | Meaning |
| --- | --- |
| `role` | Actor authority, queues, concurrency limits, legacy prose charter, structured prompt guidance, and declared non-workflow external tools |
| `artifact_kind` | Logical item with a Forge `target` (issue or PR) and `identifying_labels` |
| `state_dimension` | Named state group with an `exclusive` flag, projected as labels; states may restrict legal artifact kinds |
| `queue` | Query that selects artifacts needing attention by artifact kind(s), label filters, optional native/projected condition, and optional read-side activation policy |
| `transition` | Guarded action authorized for one or more roles; its effects may update labels, set/remove role-resolved assignees, create comments, request pull-request creation, request reviewers, submit reviews, or request PR merge |
| `gate` | Condition that unlocks another transition, either from sibling transition outcomes, a Forge-projected label/state condition, or a runtime-supplied signal such as native CI or reviews |
| `relation` | Typed link between artifacts, such as parent, dependency, or produced PR |
| `invariant` | Condition that must hold during runtime scans |
| `recovery_policy` | What to do with expired leases, partial transitions, and drift |

Labels are a portable Forge projection of workflow-owned state; native CI, dependency links, and review decisions are observed from the Forge instead of mirrored as labels. `ci_passed` and `ci_failed` are computed from fresh `CiJob` status/conclusion data, not from `testing-*` labels. A `relation` declares `{ kind, source, target }`, where `kind` is `parent`, `dependency`, or `produced_pr` and endpoints are artifact kinds. Non-label effect payloads stay portable: assignee and reviewer-request effects reference declared role ids (the runtime resolves a role to a concrete worker/user), comments carry a prose/template `body`, `create_pull_request` carries only an optional `correlation_key` while branch, title, body, labels, assignees, and (when the spec leaves it empty) a runtime correlation key come from runtime context, `submit_review` carries a portable decision, and `merge_pull_request` has no payload. The workflow layer uses native Forge dependency links for `dependency`; metadata blocks still carry correlation keys plus `parent`/`produced_pr` links and fallback dependency numbers.

The `reference-delivery.json` fixture transcribes the reference delivery design (`docs/explanation/reference-workflow.md`) into these primitives; its orthogonal lifecycle labels plus artifact-scoped state legality, non-label effects, native CI/review gates and queues, relation declarations, the relation-driven `dependency_gate`, and queue activation/matching policy validate, compile, and plan (`tests/reference_delivery.rs`). Any remaining gaps are tracked in `docs/explanation/reference-workflow-gaps.md`.

## Role prompt contract

A role may supply `prompt.guidance` and `prompt.tool_guidance`; the legacy `charter` field is still accepted and rendered as user guidance. These fields are user-authored prose. They guide behavior but do not grant workflow authority, Forge permissions, or tool access.

A role may also declare `external_tools`: each entry has an `id`, `description`, optional `required` flag, optional `constraints`, and optional `guidance`. A declaration is metadata and authority intent only. A real runner must bind a matching provider before the LLM runtime prompt/context may list the tool as available; required declarations fail worker preflight when unbound, optional unbound declarations are omitted or marked unavailable, and undeclared bindings are rejected. The conventional `coding_workspace` tool is the first executable provider: when declared and bound, the role adapter invokes a narrow workspace seam to prepare a checkout, produce a non-bookkeeping PR branch, and feed that branch into a `CreatePullRequest` transition via runtime context. Workflow state and Forge mutation still happen only through `RoleTools`/`Executor`.

Generated prompt prose is mechanical and role-id agnostic. The compiler renders workflow/role identity, concurrency, the work-item context contract, subscribed queues, authorized workflow actions, declared external-tool availability rules, and the authority boundary that executable workflow mutations come only from the compiled workflow tool manifest. Role-specific judgment such as engineering standards, review criteria, or owner values belongs in the user prompt fields or fixtures, not in compiler code or production Temper code. Production role workers call configured process responders; any fixed reference-delivery prompts are test/demo fixtures outside the production role-worker path.

## Static validation

Validation must reject or diagnose:

- duplicate role, label, artifact, state-dimension, queue, transition, or gate IDs (implemented; state ids are checked for uniqueness within each dimension, and external-tool ids are checked within each role)
- empty queue artifact-kind lists (implemented)
- references to undeclared roles, labels, artifact kinds, queues, transitions, gates, relation endpoints, or gate-condition labels/states (implemented)
- transitions whose effects contradict declared mutually exclusive dimensions (planned)
- gates that cannot be satisfied by any declared transition or external condition (planned)
- role tool declarations that exceed the role's transition authority (planned)
- unknown workflow, prompt-extension, or external-tool fields at load time through serde `deny_unknown_fields` (implemented)
- artifact mappings that cannot be represented by the Forge interface (planned)

Validation should also warn about unreachable queues, terminal states with no explanation, and labels that are declared but unused (planned).

State labels are the main cross-referenced projection of workflow state: artifact mappings, queues, state declarations, and transition effects reference label ids. State declarations may also reference legal artifact kinds, and `state_equals` conditions reference state ids within a dimension.

## Metadata block format

Workflow information that has no portable Forge field lives in a metadata block embedded in an issue or pull-request body. The block is JSON wrapped in an HTML comment:

```text
<!-- temper:workflow
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

`render_metadata_block` and `parse_metadata_block` are inverses. Parsing returns `Ok(None)` when no block is present, `Ok(Some(_))` when one parses, and `Err(MetadataError)` when a block is present but unterminated or contains invalid JSON. Relation projections (`parents`, fallback `dependencies`) accept bare Forge item numbers as the same-repository shorthand, or objects of the form `{ "repository_id": "...", "number": 34 }` for explicit cross-repository targets; classifiers type them by consulting declared `relation` primitives.

Cross-repository fan-out correlation keys use `global_child_correlation_key(parent_repo, parent_number, child_slug)`. The canonical string is `parent-repo:<repo-len>:<repo-id>#parent:<number>/child:<slug-len>:<slug>`. It is stable from only the parent artifact plus child intent, globally unique across repositories, and delimiter-safe because repository ids and slugs are length-prefixed. See [cross-repo workflow contracts](cross-repo-workflows.md) for the complete correlation and relation semantics.

## Artifact classification

`Classifier::classify_issue` and `Classifier::classify_pull_request` interpret a Forge artifact under a `ValidatedWorkflow`. Classification reads labels and the metadata block; it never mutates Forge state.

Kind resolution: when metadata names a `kind`, that kind is authoritative. Otherwise the kind is inferred from `identifying_labels` — a kind matches when all of its identifying labels are present, and the most specific match (most identifying labels) wins.

State resolution: for each dimension, the active states are those whose label is present. An exclusive dimension with more than one active state is an impossible combination. If a state declares an `artifacts` list, the classifier also rejects that state on any other artifact kind.

Relation resolution: native artifact dependency links are the source of truth for same-repository `dependency` relations. Cross-repository dependencies use the repo-qualified metadata fallback because the portable Forge dependency-link trait remains same-repository. If an artifact has no native dependencies, metadata `dependencies` is used as a compatibility fallback and may carry repo-qualified `ArtifactRef` values; if same-repository native dependencies are present, same-repo metadata dependency fallbacks are ignored while explicit repo-qualified metadata targets are still preserved. Metadata `parents` still feeds `parent` relations, and `produced_pr` declarations are read from the `parents` projection on PR artifacts. The classifier emits `ClassifiedRelation`s for declarations whose source is the artifact kind, preserving the linked `ArtifactRef` and declared possible target artifact kinds.

Success yields a `ClassifiedArtifact` (kind, target, source, optional `updated_at`, per-dimension states, parsed metadata, typed relations, raw labels). Otherwise a `ClassificationError` collects every `ClassificationDiagnostic`:

- `Unclassified`: no kind matched and metadata named none
- `AmbiguousArtifactKind`: several kinds matched equally well
- `UnknownMetadataKind`: metadata named an undeclared kind
- `TargetMismatch`: the kind maps to a different Forge target than the artifact
- `MissingIdentifyingLabel`: metadata named a kind whose identifying label is absent (drift)
- `ExclusiveStateConflict`: several states of one exclusive dimension are present
- `StateNotAllowedForArtifact`: a state label is present on an artifact kind for which that state is not legal
- `MalformedMetadata`: the body's metadata block could not be parsed

## Queue evaluation and transition planning

The planner is the pure, deterministic state-machine layer over classified artifacts. It computes the read-side parts of the runtime guarantees below (authority, preconditions, postconditions) without loading fresh state or applying effects; a later executor phase does that against the `Forge` trait.

Queue matching: a classified artifact matches a queue when its kind is one of the queue's artifact kinds, every common `labels` entry is present, either `any_of` is empty or at least one `any_of` clause's labels are all present, and any queue `condition` is satisfied by classified state or runtime signals such as `ci_failed` or `review_changes_requested`. The raw schema keeps the legacy `artifact: "code"` shorthand and also accepts `artifact: ["code", "implementation_pr"]`; `any_of` is an array of label-set objects. Because exclusive state dimensions project to mutually exclusive labels, a `code + ready` queue naturally excludes `blocked` and `in-progress` code issues. Matching does not consider activation policy.

Queue activation: a queue with no activation policy is active whenever it has at least one matched member. A queue with `min_depth` and/or `max_age` is active when it is non-empty and either its member count is at least `min_depth` or the oldest timestamped member is at least `max_age` old at `now`. `max_age` uses the classified artifact's Forge `updated_at` timestamp; snapshot-classified artifacts without timestamps cannot satisfy the age branch.

Transition planning checks, in order, and collects every problem:

- the transition is declared (else `UnknownTransition`)
- the role is authorized for the transition (else `Unauthorized`)
- the artifact's kind matches the transition's artifact kind (else `ArtifactKindMismatch`; the label/gate/state checks are skipped when the kind is wrong)
- each label effect's precondition holds: a `remove_label` target must be present (else `StalePrecondition`) and an `add_label` target must be absent (else `ContradictedPrecondition`); non-label effects have no label precondition
- every required gate is satisfied — a gate is satisfied when its condition holds (label/state, dependency, CI pass/failure, or review signal) or some satisfying transition's added labels are all present (else `GateNotSatisfied`)
- applying the effects would not leave an exclusive dimension in several states or put the resulting artifact kind into an illegal state (else `ImpossibleState`)

The impossible-state check is the plan-time complement to the planned static check on contradictory effects: even before static validation rejects such a transition, the planner refuses to plan one against a concrete artifact.

A successful plan's effects follow the transition's declared effect order. Postconditions are derived from label and assignee effects and keep that relative order, so plans are deterministic and safe for snapshot-style assertions. Comment effects do not produce postconditions because comments are append-only events rather than label-style state predicates.

## Runtime guarantees

Every transition execution must:

1. load fresh Forge state for the target artifact
2. classify it according to the validated workflow
3. compute runtime gate signals such as native dependencies, native CI, and native reviews
4. check role authority, transition preconditions, and gates
5. apply effects through an idempotent executor
6. verify postconditions or emit diagnostics

`Executor::execute` implements this loop today for labels, assignees, comments, pull-request creates, reviewer requests, review submissions, and pull-request merges. It never trusts a plan computed against stale state: it re-loads, computes gate signals (dependency status by freshly reading every dependency target in its own repository, native CI from `list_ci_jobs`, and native review status from requested reviewers plus `list_pull_request_reviews` for PRs), and re-plans against fresh state immediately before mutating. A dependency target whose repository is temporarily unreadable remains not landed for that scan. When a reference backend has an issue and pull request with the same number, dependency resolution treats the issue as authoritative and only falls back to the pull request if the issue is absent. `Executor::read_gate_signals` exposes that read-only load/classify/signal portion for runners that need conditional queue matching without mutation. The executor refuses to mutate at all if planning fails, if a plan contains an unsupported effect, if an assignee/reviewer role has no runtime user binding, if a create lacks both a spec and runtime correlation key, or if no pull-request create input is bound. It posts idempotent comments first, ensures pull requests next, requests reviewers and submits idempotently marked reviews, merges the target pull request next (skipping an already-merged target), then applies label and assignee changes together in one backend update. Because creates and merges precede the label commit point, retries can dedupe a landed create and finish post-create state, while post-merge labels become the marker that makes a retry refuse to re-run; already-merged targets are skipped, so the merge is at most once. Postconditions are verified against the artifact state returned by the commit update, so a concurrently running worker cannot make a successful transition look failed by advancing the artifact before a later reload; a mismatch yields `PostconditionFailed`.

Idempotency: re-running a label transition that already applied fails as a `Precondition` error (the source label is gone and/or the target label is present), so a retry never double-applies. `SetAssignee` is cleanly idempotent when the resolved user is already assigned, and `RemoveAssignee` is cleanly idempotent when the resolved user is already absent. `CreateComment` is guarded by a hidden marker appended to the body (`<!-- temper:comment-key=<transition>:<comment-index> -->`); the executor lists comments on the same target and skips posting when the marker already exists, so a retry after a crash-before-state-flip cannot duplicate the comment. Comments have no postcondition; instead the marker check is the verified idempotency mechanism. Idempotent artifact create is handled by `Executor::ensure_issue`, `Executor::ensure_issue_with_parent`, and `Executor::ensure_pull_request` through correlation keys. The parent-aware issue helper also repairs a found issue that has the key but lacks the required repo-qualified parent back-reference.

Agents must not mutate Forge state directly when operating under workflow control. Generated tools are the transition boundary. Runner `RoleTools::ensure_issue_in_repo` is the agent-facing cross-repo issue creation capability: it targets an explicit `RepositoryId`, checks that the worker's Forge handle can see that repository, relies on the target repo's write permission for creation, and is independent of the worker's scan shard. Runner `RoleTools::close_issue` is the narrow native lifecycle projection used by the reference workflow after a produced PR lands; it closes the issue and clears `in-progress` so completed code work no longer looks actively claimed.

## Effects

Workflow effects are the closed `plan::WorkflowEffect` enum so executors and reconcilers must handle every variant. Variants cover label add/remove, assignee set/remove, comment creation, issue and PR creation requests, reviewer requests, review submissions, lease update/release, and PR merge requests.

Transition specs now emit `AddLabel`, `RemoveLabel`, `SetAssignee`, `RemoveAssignee`, `CreateComment`, `CreatePullRequest`, `RequestReviewers`, `SubmitReview`, and `MergePullRequest`. `SetAssignee`/`RemoveAssignee` and `RequestReviewers` carry workflow role ids, not Forge user ids; `execute::ExecutionContext` resolves each role to a concrete Forge user at runtime, and missing bindings fail before mutation. `CreateComment` carries a prose/template `body`. `CreatePullRequest` carries an optional correlation key only; branch, title, body, labels, assignees, and an optional runtime correlation key come from `ExecutionContext`; a missing key after both sources are checked or a missing input fails before mutation. `SubmitReview` carries `approved`, `changes_requested`, `commented`, or `pending`. `MergePullRequest` has no payload.

Leases are not yet emitted as transition effects (`UpdateLease`/`ReleaseLease` remain placeholders). Lease changes go through `lease::LeaseManager` as standalone operations on the metadata block; see "Claims and leases".

`Executor::execute` applies `AddLabel`, `RemoveLabel`, `SetAssignee`, `RemoveAssignee`, `CreateComment`, `CreatePullRequest`, `RequestReviewers`, `SubmitReview`, and `MergePullRequest` through the `Forge` trait. `CreatePullRequest` runs before the label/assignee commit point through `Executor::ensure_pull_request`, using the effect's correlation key and the transition-bound runtime input. `RequestReviewers` calls the Forge's set-like reviewer request operation; `SubmitReview` appends a native review with an idempotency marker so retries do not duplicate workflow-submitted reviews. `MergePullRequest` merges through the Forge merge API at most once (an already-merged target is skipped) using a default merge-commit method; its transition's post-merge labels are projected as ordinary `add_label` effects in the same atomic update and act as the planner re-run guard. The executor still rejects `UpdateLease` and `ReleaseLease` with `ExecutionError::UnsupportedEffect` before any mutation.

Create effects use correlation keys for idempotent retries: `CreateIssue` requires one, while `CreatePullRequest` must have one to execute. `Executor::ensure_issue` and `Executor::ensure_pull_request` stamp the key into artifact metadata and search existing artifacts before creating. Cross-repo agent fan-out uses `RoleTools::ensure_issue_in_repo`, which delegates to `Executor::ensure_issue_with_parent` so the target issue carries both the global correlation key and a repo-qualified `parents` entry for the source artifact.

## Command journal

The command journal records the lifecycle of each runtime command so a crash between deciding to mutate and finishing the mutation is recoverable. `CommandJournal` is async with these operations: `append` (idempotent on `CommandId` — a repeated id is a no-op), `transition_state` (move to a new `CommandState` with detail and timestamp; `NotFound` for unknown ids), `get`, `list` (append order), and a defaulted `incomplete` that returns the `Planned`/`Applying` records a reconciler must investigate.

`CommandRecord::planned` constructs the initial record. `execute::Executor::execute_journaled` records `Planned` (with the previewed effects) before any mutation, advances to `Applying` immediately before applying, and finishes at `Completed` or `Failed`. If the process stops between `Applying` and the terminal update, the entry stays incomplete and the reconciler can repair it after restart.

## Claims and leases

A claim is a lease, not permanent ownership. `metadata::Lease` records role, worker or run ID, claim time, heartbeat time, and expiration time; `Lease::is_expired(now)` is true once `now >= expires_at`.

`LeasePlanner` enforces the rules: `acquire` grants when there is no lease or the existing one has expired (reclaiming the expired holder), refreshes in place when the same worker already holds an unexpired lease (preserving `claimed_at`), and fails with `LeaseConflict::HeldByOther` when a different worker holds a live lease. `heartbeat` extends the holder's lease to `now + ttl` (failing `NotHeld`/`HeldByOther` otherwise). `release` is idempotent for the holder and for an already-empty lease, and fails `HeldByOther` for a peer — forcibly clearing another worker's lease is the reconciler's job, not a peer's. `LeaseManager` applies these decisions against a `Forge` by rewriting the metadata block in a single body update.

Expired leases are handled by recovery policy. Common actions are requeue, extend, escalate, or mark for operator review.

## Reconciliation

The reconciler scans Forge artifacts and the command journal and decides what to repair or escalate; `recover::Applier` then applies the decision through the executor, lease manager, and journal (see "Applying reconciler actions"). `Reconciler::scan` is pure and deterministic: given `ArtifactSnapshot`s, `CommandRecord`s, a `DependencyStatus`, and `now`, it returns a `ReconcileReport` whose parallel `findings` and `actions` follow a stable order. `Reconciler::reconcile` is the async convenience that loads snapshots and journal entries from a `Forge` and a `CommandJournal`, derives `DependencyStatus` from native dependency links or repo-qualified metadata dependencies plus each target's fresh closed/merged state in its own repository, then calls `scan`. Child-repo read failures do not abort the whole scan; the unreadable target simply remains not landed so `Unblock` is not produced prematurely.

Findings (`ReconcileFinding`) cover: `ExpiredLease`, `ImpossibleState` (an exclusive dimension with several active states), `ClassificationDrift` (other classification failures), `BlockedWithoutDependencies` (a dependency-gated artifact that cannot be mechanically unblocked because it has no dependency relations), `PartialTransition` (a journaled command whose label effects are not all realized), `StaleCommand` (an incomplete command whose effects already landed or whose target is gone), and `DependenciesResolved` (a blocked artifact whose `dependency` relations have all landed, so its dependency-gated unblock transition can be applied). Each finding gets exactly one `RecoveryAction`: `RequeueLease`, `Escalate`, `Repair { effects }`, `MarkReconciled`, `Unblock { effects }`, or `Diagnose`. The scan order is stable: per snapshot, its expired lease then either its classification problems (when it fails to classify) or its mechanical dependency diagnostics/unblocks (when it classifies cleanly), in snapshot order, then incomplete journal commands in journal order.

`RecoveryPolicy` is the hook layer: one defaulted method per finding class, so a workflow overrides only what it needs. `DefaultRecoveryPolicy` requeues expired leases, escalates impossible states and drift, records a named diagnosis for zero-dependency blocked artifacts, repairs partial transitions with their pending effects, marks stale commands reconciled, and mechanically unblocks dependency-gated work (`on_resolved_dependencies` → `Unblock`) once its prerequisites land. Still planned for the reconciler: duplicated correlation keys, missing required relations, merged PRs whose linked code issue stays open, and validation-failure labels on merged PRs.

### Applying reconciler actions

`recover::Applier` turns the decided actions into mutations, routing each through the component that already owns the matching path rather than re-implementing one. `Applier::apply_report` walks the report's parallel findings and actions and:

- `RequeueLease` → `LeaseManager::clear`, the reconciler's authority path that force-clears a presumed-gone holder's lease (unlike `release`, it does not check the holder) conditionally on the load-time version.
- `Repair` → `Executor::apply_label_effects`, the executor's idempotent label-apply path (load fresh, apply only the not-yet-realized labels in one update, verify), then marks the originating journal command `Reconciled`.
- `Unblock` → the same idempotent label apply, journaling a fresh `Planned`→`Applying`→`Completed` command (id derived from target and transition) so a crash mid-apply is recoverable; a terminal command is skipped on a later pass.
- `MarkReconciled` → `CommandJournal::transition_state` to `Reconciled`.
- `Escalate`/`Diagnose` → recorded in `ApplyOutcome::advisory`; the applier performs no Forge mutation, so an escalation is never silently turned into a label or comment change.

Every mutating action loads fresh state and applies at most once, so re-running the same report is a no-op rather than a double-apply, and running scan→apply to a fixpoint converges. `ApplyOutcome` partitions what was `applied` versus left `advisory`; `ApplyError` separates executor, lease, and journal failures.

## Compilation outputs

`compile::compile` produces a `CompiledWorkflow` with:

- `RoleManifest` per role, embedding its `PromptManifest`, user prompt extension, declared `ExternalToolManifest` entries, and role-specific workflow `tools`
- `ToolManifest` entries (intent-level, one per authorized transition)
- `QueueManifest` entries with subscribers, multi-kind/disjunctive filters, and activation policy, for runtime queue evaluation
- `LabelManifest` (a list of `LabelSpec` with `LabelUsage` annotations) for Forge label setup
- `TransitionManifest` entries forming the runtime transition table

Still planned: optional generated Rust code for statically checked workflows, and generated tool bodies that enforce preconditions and apply effects.

Generated tools expose intent-level operations such as `claim_code` or `address_ci_failure`, not generic Forge mutation operations. Each `ToolManifest` is named after its transition and carries that transition's artifact, required gates, and effects.
