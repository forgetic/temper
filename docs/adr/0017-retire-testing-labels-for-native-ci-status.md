# ADR 0017: Retire testing labels in favor of native CI status

## Status

Accepted

## Context

The reference workflows still carried `testing-passed` and `testing-failed`
labels plus a tester role, even after ADR 0014 taught the merge gate to read
native CI jobs through `ci_passed`. Those labels duplicated a fact the Forge
already owns: the current CI status for the pull request head. GitHub and
Forgejo both expose that status natively.

Duplicating CI as workflow labels creates drift: a label can remain green after
a new commit, or stay red after CI is rerun. It also forces cleanup transitions
whose only job is to mirror provider state.

## Decision

- Remove workflow-owned testing outcome labels and the manual `testing_gate`
  from the reference fixtures.
- Keep merge eligibility derived from native gates, now `review_gate AND
  ci_gate` for the reference workflows.
- Add a `ci_failed` condition alongside `ci_passed` so queues can route failed
  CI back to the engineer without storing `testing-failed`.
- Extend `CiStatus` from a pass boolean to an aggregate state. The original
  `Pending`, `Passed`, and ordinary `Failed` states are now joined by
  `RecoveryRequired`, which keeps non-repairable terminal results red without
  selecting `ci_failed`. Runtime code computes it from fresh `CiJob`s.

The portable aggregation rule remains conservative: reduce jobs to the latest
job per name. CI passes when that set is non-empty and every latest job is
completed with `Success`. It is an ordinary failure only when every latest job
is completed and at least one explicitly reports `Failure`; other terminal
categories require recovery. No jobs or in-flight jobs are pending/unknown.

## Consequences

- `testing-passed`, `testing-failed`, `needs-testing`, tester queues, and
  `record_test_*` transitions are retired from the fixtures.
- `pr_ci_failed` is a native-signal queue condition, not a label filter, and
  requires explicit ordinary failure evidence. Interrupted or otherwise
  non-repairable terminal results use the distinct `ci_recovery_required`
  signal.
- Provider models expose typed CI status/conclusion plus bounded original
  provider conclusion/reason and run/attempt identity. The aggregate uses typed
  categories for routing and retains original fields for diagnostics.
- Agents and runners must treat CI as observed native state. They should not add
  labels that mirror CI outcomes.
