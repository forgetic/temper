# Reference workflow: gap record

This page records the expression and runtime gaps discovered while transcribing
`reference-workflow.md` into `crates/harness-workflow/fixtures/reference-delivery.json`
and exercising it through `tests/reference_delivery.rs`.

The label-state-machine core — roles, type/state labels, artifact-scoped state
legality, exclusive and non-exclusive dimensions, queues with activation and
richer matching, role-authorized label transitions, transition-satisfied gates,
external-signal gates, relation declarations, the relation-driven
`dependency_gate`, non-label
effects for assignees/comments/PR create/reviewer requests/review submissions/merge,
and native dependency links plus native reviews — **validates, compiles, and
plans today**.

## Resolved expression gaps

- **Non-label effect expression/execution.** Assignee, comment, pull-request
  create, and pull-request merge effects are modeled and the executor applies
  the supported effects idempotently.
- **Pull-request idempotent create.** `CreatePullRequest` runs through
  `Executor::ensure_pull_request` with a correlation key and runtime create
  input.
- **External-signal gates.** `ci_gate` now uses the `ci_passed` condition fed by
  native Forge CI jobs, and `review_gate` uses `review_approved` fed by native
  pull-request reviews; the old adapter-projected `ci = passed` and review
  result labels/state are retired.
- **Relation primitive.** The spec declares `parent`, `dependency`, and
  `produced_pr` relations; classifiers type native dependency links and metadata
  projections using those declarations.
- **Relation-driven dependency unblocking.** The `dependencies_resolved` gate
  condition powers a `dependency_gate`; `Planner::dependency_unblocks` and a
  reconciler `DependenciesResolved` finding / `Unblock` action clear
  `blocked` mechanically once every prerequisite has landed. The
  runtime derives landed status from native Forge dependency targets (closed
  issues or merged pull requests), like the CI signal.
- **Applying reconciler actions.** `recover::Applier` applies a
  `ReconcileReport` through the existing runtime components: lease clears go
  through `LeaseManager::clear`, label repairs and the mechanical unblock reuse
  the executor's idempotent label-apply path, and journal transitions go through
  the command journal. Applying is idempotent and crash-safe, and the scan→apply
  loop converges.
- **Queue activation policy.** `RawQueue`/`ValidatedQueue` carry optional
  `min_depth`/`max_age`; `owner_alignment` uses the planner's pure activation
  predicate for cohorts.
- **Multi-artifact-kind queues.** A queue can now select several artifact kinds.
  `needs_architect` covers issue artifact kinds and `implementation_pr` without
  duplicate per-kind queues; `needs_owner`/`needs_human` cover the design-issue
  feedback handoff.
- **Disjunctive queue label-sets and native queue conditions.** Queue `labels`
  remain an AND filter and `any_of` adds OR branches. The review return queue
  now keys off native `review_changes_requested`; testing failure remains a
  label-routed queue.

ADR 0012 covers the queue primitive extensions as one decision: Phase 13's
activation policy plus Phase 14's multi-kind and disjunctive matching.

## Remaining limitations

- **Claim-time lease effects.** Claims can be represented with
  `metadata::Lease` and managed through `LeaseManager`, but transition specs do
  not yet emit lease effects inside `Executor::execute`.
- **Escalation/diagnosis projection.** `recover::Applier` applies the mutating
  recovery actions (including the mechanical `Unblock`), but `Escalate` and
  `Diagnose` stay advisory: projecting them into a label or comment is left to a
  workflow-specific adapter rather than being decided in the runtime layer.
