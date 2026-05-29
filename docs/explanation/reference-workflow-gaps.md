# Reference workflow: confirmed gap backlog

This page records the gaps between the reference delivery design
(`reference-workflow.md`) and what the current `harness-workflow` spec types,
compiler, and planner can express and run. It was produced by transcribing the
design into `crates/harness-workflow/fixtures/reference-delivery.json` and
running it through validate / compile / plan
(`crates/harness-workflow/tests/reference_delivery.rs`).

The label-state-machine core — roles, type/state labels, exclusive and
non-exclusive dimensions, queues with activation policy, role-authorized label
transitions, transition-satisfied gates, external-signal gates, and relation
declarations — **validates, compiles, and plans today**. The gaps below are
everything the design still needs beyond that core.

## What the fixture had to work around

One design decision still cannot be transcribed faithfully and is encoded as a
workaround, which is itself a gap:

- **Work-return and flag queues split by hand.** The design's single
  `pr_changes_requested` queue (changes-requested **or** testing-failed) became
  two queues; `escalated`/`needs-human` were bound to `implementation_pr` only.
  See P2 items below.

## Resolved gaps

- **External-signal gates (resolved in Phase 11).** `ci_gate` is now a
  `state_equals` external condition over `ci = passed`, with CI projected into
  labels/state by an adapter outside `harness-forge`. The zero-role
  `record_ci_pass`/`record_ci_failure` workaround was removed from the fixture.
- **Relation primitive (resolved in Phase 12a).** The spec now declares
  `parent`, `dependency`, and `produced_pr` relations between artifact kinds;
  classifiers type metadata `parents`/`dependencies` using those declarations.
- **Queue activation policy (resolved in Phase 13).** `RawQueue`/`ValidatedQueue`
  now carry optional `min_depth`/`max_age`; `owner_alignment` uses the planner's
  read-side activation predicate for cohorts.

## Prioritized backlog

### P0 — the delivery loop cannot complete without these

1. **Claim-time lease effects.** The spec can express and execute assignee,
   comment, pull-request create, and pull-request merge effects. `CreatePullRequest`
   runs through `Executor::ensure_pull_request` with a correlation key and
   runtime create input, and `MergePullRequest` is at-most-once with the
   post-merge `landed`/`owner-pending` projection modeled as `add_label`
   effects. Claim-time lease effects are still not emitted.

### P1 — modeled behavior is unsafe or under-gated without these

2. **`dependency_gate` over declared relations.** The relation primitive is now
   present, but the code→code dependency is not yet used as a gate. The
   `dependency_gate` remains omitted and `mark_code_ready` is a bare architect
   label flip with no prerequisite check. Needs a relation-driven
   `dependency_gate` and a reconcile action for mechanical unblocking.

### P2 — fidelity/efficiency; expressible workarounds exist

3. **Multi-artifact-kind queues.** A queue matches exactly one `artifact_kind`,
   but `escalated`/`needs-human` can sit on issues *or* PRs. *Workaround in
   fixture:* bound `escalations`/`needs_human` to `implementation_pr` only;
   issue-level routing would need duplicate per-kind queues. **Not in the
   design's anticipated list.**
4. **Disjunctive (OR) queue label-sets.** Queue matching is AND-only, but the
   single conceptual "return to engineer" queue is changes-requested **or**
   testing-failed. *Workaround in fixture:* two queues
   (`pr_changes_requested`, `pr_testing_failed`), engineer subscribed to both.
   **Not in the design's anticipated list.**

## Corrections to the design's anticipated list

The design's "Anticipated harness additions" listed the P0/P1 items above plus
queue activation. External-signal gates, pull-request create/idempotency, the
relation primitive, and queue activation are now resolved. Two further gaps were
hit that the list did not mention: multi-artifact-kind queues and disjunctive
queue label-sets. Both extend the queue primitive.

## Decisions recorded

ADR 0012 covers the queue primitive extensions as one decision: Phase 13's
activation policy plus Phase 14's multi-kind and disjunctive matching.
