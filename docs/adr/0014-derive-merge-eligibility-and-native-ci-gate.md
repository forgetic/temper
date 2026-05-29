# ADR 0014: Derive merge eligibility from gates and read native CI

## Status

Accepted. Updated by ADR 0017, which removes the separate workflow-owned
`testing_gate` and uses native CI status for both pass and failure routing.

## Context

Merge eligibility was modeled as `merge-ready`, a workflow-owned label: the
`approve_merge` transition wrote it and the workflow then treated it as the
"may merge" fact. That duplicates state the gates already encode. The gates
(review, testing, CI) are the real preconditions; a separate stored label can
drift from them, must be cleaned up, and adds nothing the gate evaluation does
not already prove.

CI was modeled as an *external-signal gate* (ADR 0010): an adapter observed the
provider CI result and projected it into a portable `ci` state dimension backed
by `ci-pending`/`ci-passed`/`ci-failed` labels, and `ci_gate` was a
`state_equals { dimension: ci, state: passed }` condition. But `harness-forge`
already exposes a portable, native CI model — `list_ci_jobs`, `CiJob`,
`CiJobStatus`, `CiJobConclusion` — so the adapter-projected labels are a second,
redundant representation of a fact the Forge already owns. The roadmap
(`docs/explanation/native-forge-state-roadmap.md`, Phase A) calls for promoting
CI from projected/owned state to observed/native state.

The dependency gate (ADR 0011/0012, `dependencies_resolved`) already shows the
pattern for a gate condition fed by a fact the pure planner must not derive
itself: the runtime supplies a small **signal** (`DependencyStatus`) computed
from fresh Forge state, and the planner only reads it. CI fits the same shape.

## Decision

1. **`merge-ready` ceases to be owned state.** No transition writes it and no
   gate reads it. Merge eligibility is *derived*: the merge transition is gated
   on the AND of the review, testing, and CI gates. The reference fixtures drop
   the `merge-ready` label and the `merge` state dimension.

2. **Merge eligibility is the AND of review/testing/CI gates.** This is the
   existing `requires_gates` mechanism on `approve_merge`; no new primitive is
   needed. A merge plans only when every required gate is satisfied.

3. **Merge at-most-once relies on native `merged` state plus the post-merge
   `landed` projection.** The executor already merges before the label commit
   point and skips an already-`merged` pull request, so the merge itself is
   at-most-once regardless of any label. The post-merge `landed`/`alignment`
   labels (ordinary `add_label` effects on `approve_merge`) remain the planner's
   re-run guard: once `landed` is present, re-planning `approve_merge` fails its
   `add_label landed` precondition, so a completed merge is never re-run. This
   replaces `merge-ready`'s former double duty as the re-run guard.

4. **The CI gate reads native `CiJob` conclusions.** A new `ci_passed` gate
   condition (no payload — it references the artifact's CI the way
   `dependencies_resolved` references its relations) is evaluated against a
   runtime-supplied CI signal. The `ci` state dimension and the
   `ci-pending`/`ci-passed`/`ci-failed` adapter labels are retired from the
   fixtures.

### Runtime signal shape: `GateSignals`

Rather than add a second standalone signal parameter, the runtime signals are
bundled into a single `plan::GateSignals { dependencies: DependencyStatus, ci:
CiStatus }`, and `Planner::plan_transition_with` takes `&GateSignals`. This is
justified over a bare `CiStatus` parameter because the roadmap adds a third
runtime signal next (native reviews, Phase C); a bundle absorbs each new signal
without re-threading every planner call site, and it keeps one obvious place to
construct "the facts the runtime read fresh before planning". `Planner` stays
pure: it only reads `signals.ci().is_passed()` and `signals.dependencies()`, and
never lists jobs or talks to a Forge.

`CiStatus` is a thin verdict (`{ passed: bool }`), mirroring how
`DependencyStatus` is a thin set the planner only tests membership against. The
**pass rule** is computed by the runtime, not the planner, via
`CiStatus::from_jobs(&[CiJob])`:

> Reduce the jobs to the latest job per name (by `created_at`). CI is *passed*
> when that set is non-empty and every latest-per-name job has status
> `Completed` with conclusion `Success`. Any latest job that is still
> `Queued`/`Running`, or concluded anything other than `Success` (`Failure`,
> `Cancelled`, `TimedOut`, `Skipped`, `Neutral`), leaves CI *not passed*.

The rule is deliberately conservative: an in-flight or non-success latest job
blocks the merge gate, and a pull request with no CI jobs is *not passed* (you
do not merge before CI has run). The executor computes the signal from
`list_ci_jobs` for the pull request (filtered by the PR's head commit when the
backend records one) and threads it into `plan_transition_with`. Issues get an
empty signal because they carry no CI.

### Why not extend `harness-forge`

The portable CI model already carries everything the pass rule needs (per-job
name, status, conclusion, and PR/commit association). Phase A *reuses* it and
adds no provider-specific fields. If a future provider rule needs more (for
example required-checks configuration), that is a separate, justified Forge
change — not smuggled in here.

## Consequences

- The merge gate is a single source of truth: review and native CI are read
  fresh. There is no stored `merge-ready` to drift or to clean up. ADR 0017
  later retired the separate testing labels/gate.
- CI is observed, not projected. The `record_ci_*` adapter transitions were
  already removed (ADR 0010); now the adapter labels and the `ci` dimension go
  too. An adapter that wrote `ci-passed` is no longer part of the contract; the
  runtime reads `CiJob` conclusions directly.
- `Planner::plan_transition_with` now takes `&GateSignals`. Callers that only
  cared about dependencies wrap their `DependencyStatus` in `GateSignals`;
  `plan_transition` (no signals) and the reconciler's dependency-unblock path
  are unchanged in spirit (they construct a `GateSignals` with default CI).
- The merge at-most-once safety argument is unchanged in mechanism (native
  `merged` skip + post-merge projection guard) but no longer mentions
  `merge-ready`; `docs/reference/robustness-guarantees.md` is updated to match.

## Alternatives considered

- **Keep `merge-ready` as a derived cache.** Rejected: a cache of a fact the
  gates already prove only adds drift and cleanup with no read-side benefit; the
  planner re-evaluates gates against fresh state anyway.
- **A bare `CiStatus` parameter instead of `GateSignals`.** Rejected for the
  re-threading cost once reviews (Phase C) add a third signal; the bundle is the
  smaller long-run change and documents the runtime-signal boundary in one type.
- **Compute the CI pass rule inside the planner.** Rejected: it would make the
  planner read provider-shaped job lists and break the "planner is pure, runtime
  supplies signals" rule that the dependency gate already established.
