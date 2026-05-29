# Reference workflow: gap record

This page records the expression and runtime gaps discovered while transcribing
`reference-workflow.md` into `crates/harness-workflow/fixtures/reference-delivery.json`
and exercising it through `tests/reference_delivery.rs`.

The label-state-machine core — roles, type/state labels, exclusive and
non-exclusive dimensions, queues with activation and richer matching,
role-authorized label transitions, transition-satisfied gates, external-signal
gates, relation declarations, non-label effects for assignees/comments/PR
create/merge, and metadata-projected relations — **validates, compiles, and
plans today**.

## Resolved expression gaps

- **Non-label effect expression/execution.** Assignee, comment, pull-request
  create, and pull-request merge effects are modeled and the executor applies
  the supported effects idempotently.
- **Pull-request idempotent create.** `CreatePullRequest` runs through
  `Executor::ensure_pull_request` with a correlation key and runtime create
  input.
- **External-signal gates.** `ci_gate` is a `state_equals` external condition
  over `ci = passed`; adapters project CI into labels/state outside
  `harness-forge`.
- **Relation primitive.** The spec declares `parent`, `dependency`, and
  `produced_pr` relations; classifiers type metadata projections using those
  declarations.
- **Queue activation policy.** `RawQueue`/`ValidatedQueue` carry optional
  `min_depth`/`max_age`; `owner_alignment` uses the planner's pure activation
  predicate for cohorts.
- **Multi-artifact-kind queues.** A queue can now select several artifact kinds.
  `escalations` and `needs_human` cover issue artifact kinds and
  `implementation_pr` without duplicate per-kind queues.
- **Disjunctive queue label-sets.** Queue `labels` remain an AND filter and
  `any_of` adds OR branches. The single `pr_changes_requested` queue now routes
  both `review-changes-requested` and `testing-failed` PRs.

ADR 0012 covers the queue primitive extensions as one decision: Phase 13's
activation policy plus Phase 14's multi-kind and disjunctive matching.

## Remaining limitations

- **Claim-time lease effects.** Claims can be represented with
  `metadata::Lease` and managed through `LeaseManager`, but transition specs do
  not yet emit lease effects inside `Executor::execute`.
- **Relation-driven dependency unblocking.** Relations are typed, but the
  `dependency_gate` and mechanical reconcile action remain future work (roadmap
  Phase 12b).
