# Reference workflow: confirmed gap backlog

This page records the gaps between the reference delivery design
(`reference-workflow.md`) and what the current `harness-workflow` spec types,
compiler, and planner can express and run. It was produced by transcribing the
design into `crates/harness-workflow/fixtures/reference-delivery.json` and
running it through validate / compile / plan
(`crates/harness-workflow/tests/reference_delivery.rs`).

The label-state-machine core — roles, type/state labels, exclusive and
non-exclusive dimensions, queues, role-authorized label transitions,
transition-satisfied gates, and external-signal gates — **validates, compiles,
and plans today**. The gaps below are everything the design still needs beyond
that core.

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

## Prioritized backlog

### P0 — the delivery loop cannot complete without these

1. **Claim-time lease effects.** The spec can express and execute assignee,
   comment, pull-request create, and pull-request merge effects. `CreatePullRequest`
   runs through `Executor::ensure_pull_request` with a correlation key and
   runtime create input, and `MergePullRequest` is at-most-once with the
   post-merge `landed`/`owner-pending` projection modeled as `add_label`
   effects. Claim-time lease effects are still not emitted.

### P1 — modeled behavior is unsafe or under-gated without these

2. **First-class relations + `dependency_gate`.** Relations exist only in
   metadata (`parents`/`dependencies`), not as a spec primitive. The code→code
   dependency could not be expressed, so `dependency_gate` was omitted and
   `mark_code_ready` is a bare architect label flip with no prerequisite check.
   Needs the `relation` primitive, a relation-driven `dependency_gate`, and a
   reconcile action for mechanical unblocking.

### P2 — fidelity/efficiency; expressible workarounds exist

3. **Queue activation policy (`min_depth`/`max_age`).** `RawQueue` has no
   activation fields, so `owner_alignment` is a plain per-item queue instead of
   a batched cohort. A read-side spec field plus a pure planner predicate.
   *Workaround in fixture:* plain queue.
4. **Multi-artifact-kind queues.** A queue matches exactly one `artifact_kind`,
   but `escalated`/`needs-human` can sit on issues *or* PRs. *Workaround in
   fixture:* bound `escalations`/`needs_human` to `implementation_pr` only;
   issue-level routing would need duplicate per-kind queues. **Not in the
   design's anticipated list.**
5. **Disjunctive (OR) queue label-sets.** Queue matching is AND-only, but the
   single conceptual "return to engineer" queue is changes-requested **or**
   testing-failed. *Workaround in fixture:* two queues
   (`pr_changes_requested`, `pr_testing_failed`), engineer subscribed to both.
   **Not in the design's anticipated list.**

## Corrections to the design's anticipated list

The design's "Anticipated harness additions" listed the P0/P1 items above plus
queue activation. External-signal gates and pull-request create/idempotency are
now resolved. Two further gaps were hit that the list did not mention: items 4
(multi-artifact-kind queues) and 5 (disjunctive queue label-sets). Both extend
the queue primitive.

## Decisions to flag (no ADR written yet)

Several backlog items extend the spec primitives and may each warrant an ADR.
These are surfaced for a human decision, not decided here:

- **Queue activation policy (item 3)** — new queue fields; extends the queue
  primitive.
- **Queue matching extensions (items 4–5)** — multi-kind and disjunctive
  matching also extend the queue primitive; open question whether they fold
  into the activation-policy ADR or stand alone.
