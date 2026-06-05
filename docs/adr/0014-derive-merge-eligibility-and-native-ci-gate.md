# ADR 0014: Derive merge eligibility from gates and read native CI

## Status

Accepted. ADR 0017 later removed the separate workflow-owned testing gate and
uses native CI for both pass and failure routing.

## Context

Merge eligibility used to be stored as a workflow-owned `merge-ready` label. That
label duplicated the actual preconditions: review, testing, and CI gates. A
stored cache can drift, must be cleaned up, and does not add proof beyond gate
evaluation.

CI was also projected into workflow-owned labels (`ci-pending`, `ci-passed`,
`ci-failed`) even though `temper-forge` already exposes portable CI jobs and
conclusions. The dependency gate had established the better pattern: the runtime
reads fresh Forge state and supplies a small signal to the pure planner.

## Decision

1. **Retire `merge-ready` as workflow-owned state.** No transition writes it and
   no gate reads it. A role-routing label may request owner attention, but it is
   not proof that merge gates pass.
2. **Derive merge eligibility from gates.** The merge transition requires the
   review/testing/CI gates that are still part of the workflow. A merge plans
   only when all required gates are satisfied.
3. **Read CI as native Forge state.** `ci_passed` is a gate condition evaluated
   from runtime-supplied CI status, not from workflow-owned labels. The runtime
   computes that status from `list_ci_jobs` for the pull request, filtered to the
   current head when the backend supplies one.
4. **Keep the planner pure.** Runtime facts are bundled in `GateSignals`; the
   planner only tests the supplied dependency/CI/review verdicts and never reads
   Forge jobs itself.
5. **Keep merge at-most-once via native merged state and post-merge labels.** The
   executor skips already merged PRs. Ordinary `landed` / `alignment` add-label
   effects remain the planner re-run guard after projection.

The CI pass rule is conservative: consider the latest job per name; CI passes
only when that set is non-empty and every latest job is completed with success.
Queued, running, missing, or non-success latest jobs do not satisfy the pass
gate.

## Consequences

- Review and CI gates are the single source of truth for merge eligibility.
- The reference fixtures no longer carry `merge-ready` or CI status labels.
- CI adapters no longer project pass/fail labels; the runtime observes native
  `CiJob` status and conclusion data.
- Existing merge retry safety still holds: native merged-state skip plus
  post-merge label projection prevents duplicate merges and preserves `landed` /
  `alignment` on closed PRs.
- Future provider-specific requirements, such as branch protection or required
  checks configuration, need separate Forge modeling rather than hidden planner
  assumptions.

## Alternatives considered

- **Keep `merge-ready` as a derived cache.** Rejected because it adds drift and
  cleanup without changing the gate proof.
- **Pass `CiStatus` separately instead of using `GateSignals`.** Rejected because
  the bundle accommodates dependencies, CI, and reviews without re-threading
  every planner API for each signal family.
- **Compute the CI pass rule inside the planner.** Rejected because the planner
  must stay pure and provider-agnostic.
