# Workflow runtime robustness guarantees

This page records the robustness properties the workflow runtime is tested to
uphold, the deterministic test support that proves them, and the limitations the
tests surfaced. It complements `workflow-layer.md`, which defines the runtime
contract; this page is the safety-property register.

All guarantees are proven by deterministic tests (Phase 8). No test sleeps or
depends on wall-clock time: faults fire on fixed call counts and every timestamp
is supplied explicitly, so the suite is reproducible.

## Test support

- `tests/support/crash.rs` — `CrashForge<F>` wraps any `Forge` and injects a
  deterministic fault on a chosen 1-based call to a chosen operation:
  - `FaultPoint::Before` fails before delegating, so the backend is never
    touched (a crash on the way into a side effect).
  - `FaultPoint::After` delegates first and then returns an error, so the
    mutation lands but the caller observes failure (a crash right after the side
    effect — the case retries must tolerate).
- `tests/crash_injection.rs` — crash-before/after, a fault matrix, journaled
  restart recovery, duplicated tool calls, and interleaved workers.
- `tests/safety_properties.rs` — the safety assertions below.

## Proven safety properties

| Property | Where proven |
| --- | --- |
| No duplicate artifact is created for one correlation key, even when a create crashes after it lands | `no_duplicate_artifact_is_created_for_a_correlation_key_after_a_crash` |
| An exclusive claim never holds two active leases at once | `an_exclusive_claim_never_has_two_active_leases` |
| A merge is not authorized, and the pull request is not merged, until review and testing gates pass; once gated, it merges and projects `landed`/`owner-pending` | `a_merge_is_not_authorized_until_review_and_testing_gates_pass` |
| The gate mechanism blocks a merge until CI, review, and testing all pass | `the_merge_gate_mechanism_requires_ci_review_and_testing_together` |
| A gated merge executes at most once: a crash that lands the merge but loses the response is retried without merging twice | `a_merge_executes_at_most_once_under_retry` |
| A failed review gate returns work to the engineer, and the reviewer cannot perform that return path | `a_failed_review_gate_returns_work_to_the_engineer` |
| Expired in-progress work becomes visible for recovery | `expired_in_progress_work_becomes_visible_for_recovery` |
| Impossible label combinations are detected by both the executor and the reconciler | `impossible_label_combinations_are_detected_not_silently_ignored` |

## Crash, retry, and restart behavior

- **Atomic label application.** The executor folds all of a transition's label
  effects into one `update_issue`/`update_pull_request` call, so a crash cannot
  split a transition's labels. After any single-write fault the artifact is
  either fully transitioned or untouched, never half-labeled.
- **Crash before an effect.** State is intact and a retry completes.
- **Crash after an effect.** The effect landed once; a retry re-loads fresh
  state, finds the precondition stale, and refuses to apply the transition
  again. Retries never double-apply.
- **Journaled restart.** A command interrupted between `Applying` and a terminal
  state is recoverable after a restart: if its effects never landed it is a
  `PartialTransition` to `Repair`; if they already landed it is a `StaleCommand`
  to `MarkReconciled`.
- **Idempotent create.** `Executor::ensure_issue` stamps the correlation key into
  the new body before creating, so a create that crashes after it lands is found
  by the retry instead of being duplicated.
- **At-most-once merge.** `MergePullRequest` runs before the label commit point
  and is skipped when the freshly loaded pull request is already merged. A crash
  that lands the merge but loses the response leaves the post-merge labels
  unapplied; the retry observes the merged state, skips the merge, and finishes
  the `landed`/`owner-pending` projection, so the merge happens exactly once and
  the post-merge labels survive on the closed pull request.

## Limitations discovered by the tests

These are real gaps the Phase 8 tests exposed; they are documented here rather
than hidden:

- **Lease acquisition is not compare-and-swap.** `LeaseManager` loads fresh
  state, plans, then writes. A live lease cannot be taken by a peer (tested), and
  the metadata holds a single lease, so two *recorded* leases are structurally
  impossible. But two workers that both load "no lease" before either writes can
  still produce a lost update (last write wins). Closing that window needs an
  atomic compare-and-swap or conditional update primitive that the portable
  `Forge` interface does not yet expose. The deterministic tests serialize
  backend calls and assert the single-holder outcome for the tested
  interleavings. The webhook-accelerated triggering model (ADR 0009) widens this
  concurrency window, so closing it with a compare-and-swap primitive is a
  prioritized follow-up.
- **Pull-request idempotent create is not implemented.** Only
  `Executor::ensure_issue` exists. The correlation-key mechanism is identical for
  pull requests, so the no-duplicate guarantee will transfer once it is added.
- **The five-role fixture wires no CI gate.** It declares `ci-passed`/`ci-failed`
  labels and a `ci` state dimension, but `approve_merge` requires only
  `review_gate` and `testing_gate`. CI is modeled as external state with no
  gating transition. The CI gate property is proven instead over an inline
  three-gate workflow that shows the mechanism is identical.
- **Testing failure has no modeled return path.** `record_test_failure` sets
  `testing-failed`, but no transition clears it back to `needs-testing`. The
  review path (`request_changes` → `address_review_changes`) is the modeled
  failed-gate return path; the testing path would need an equivalent transition.
- **Reconciler actions are decided, not applied.** `Reconciler::scan` and
  `reconcile` choose `RecoveryAction`s; applying them through the executor and
  lease manager is left to the caller and is not yet automated.

## Out of scope

No real LLM or agent-provider integration is exercised. Tests drive the
deterministic in-memory backend through the workflow runtime only.
