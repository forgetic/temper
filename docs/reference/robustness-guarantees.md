# Workflow runtime robustness guarantees

This page records the robustness properties the workflow runtime is tested to
uphold, the deterministic test support that proves them, and the limitations the
tests surfaced. It complements `workflow-layer.md`, which defines the runtime
contract; this page is the safety-property register.

All guarantees are proven by deterministic tests (Phases 8–10). No test sleeps or
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
  restart recovery, duplicated tool calls, interleaved workers, and retry-safe
  application of reconciler repair/unblock actions.
- `tests/recovery.rs` — applying reconciler actions through the runtime: lease
  clear, label repair/unblock, journal-state transitions, advisory escalation,
  no-op re-apply, and scan→apply convergence.
- `tests/safety_properties.rs` — the safety assertions below.

## Proven safety properties

| Property | Where proven |
| --- | --- |
| No duplicate issue or pull request is created for one correlation key, even when a create crashes after it lands | `no_duplicate_artifact_is_created_for_a_correlation_key_after_a_crash` |
| An exclusive claim never holds two active leases at once | `an_exclusive_claim_never_has_two_active_leases` |
| Two acquirers that both observe "no lease" cannot both win: lease acquisition is a compare-and-swap, so the loser observes a conflict | `two_no_lease_acquirers_cannot_both_win_the_same_claim`, `interleaved_acquirers_cannot_both_win_the_same_unclaimed_issue` |
| A merge is not authorized, and the pull request is not merged, until native review approval and CI gates pass; once gated, it merges and projects `landed`/`alignment` | `a_merge_is_not_authorized_until_review_and_ci_gates_pass` |
| The gate mechanism blocks a merge until native CI conclusions plus native review approval both pass | `the_merge_gate_mechanism_requires_ci_and_review_together`, `ci_gate_reads_native_ci_job_conclusions` |
| A gated merge executes at most once: a crash that lands the merge but loses the response is retried without merging twice | `a_merge_executes_at_most_once_under_retry` |
| A failed review gate returns work to the engineer, and the reviewer cannot perform that return path | `a_failed_review_gate_returns_work_to_the_engineer` |
| Expired in-progress work becomes visible for recovery | `expired_in_progress_work_becomes_visible_for_recovery` |
| Impossible label combinations are detected by both the executor and the reconciler | `impossible_label_combinations_are_detected_not_silently_ignored` |
| Reconciler actions are applied through the runtime and re-applying a report is a no-op | `recovery.rs` (`requeue_lease_clears_the_lease_through_the_manager`, `repair_realizes_pending_labels_and_reconciles_the_command`, `unblock_realizes_labels_and_journals_a_completed_command`, `re_applying_a_report_is_a_no_op`) |
| Cross-repo dependency aggregation unblocks a parent only after every child has landed in its own repository; a transient child read failure is not a false unblock | `dependency_aggregation.rs` |
| Applying a repair/unblock is retry-safe across a crash before or after the write | `crash_injection.rs` (`applying_a_repair_is_retry_safe_under_a_crash`, `applying_an_unblock_is_retry_safe_under_a_crash`) |
| The scan→apply loop converges to a clean state | `recovery.rs` (`the_scan_apply_loop_converges_to_a_clean_state`) |

## Lease acquisition is compare-and-swap

Lease acquisition closes its lost-update window with the portable
optimistic-concurrency primitive (ADR 0013, `docs/reference/forge-interface.md`).
`LeaseManager` captures the artifact's `Version` at load time and writes the
lease conditionally on it (`expected_version`), so the read-then-write gap is no
longer a lost update:

- A live lease still cannot be taken by a peer (the planner refuses it).
- Two workers that both load "no lease" before either writes can no longer both
  win. The first conditional write advances the version; the second write, made
  against the now-stale captured version, fails its compare-and-swap and the
  loser observes `LeaseError::Contended`. `acquire`, `heartbeat`, and `release`
  are all conditional.
- This is proven deterministically by capturing the load-time token — not by
  hand-ordering the writes: `prepare_acquire` loads and plans, `commit` performs
  the conditional write, and the tests interleave two acquirers (A-load, B-load,
  A-write, B-write) so the second commit must lose. See
  `two_no_lease_acquirers_cannot_both_win_the_same_claim`
  (`tests/safety_properties.rs`) and
  `interleaved_acquirers_cannot_both_win_the_same_unclaimed_issue`
  (`tests/leases.rs`), with backend-level conditional-write conflict tests in
  both reference backends.

The webhook-accelerated triggering model (ADR 0009) widens the concurrency
window, but a wider window can no longer produce a lost-update lease race.

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
- **Idempotent create.** `Executor::ensure_issue`,
  `Executor::ensure_issue_with_parent`, and `Executor::ensure_pull_request`
  stamp the correlation key into the new body before creating, so an issue or PR
  create that crashes after it lands is found by the retry instead of being
  duplicated. Normal retries use bounded summary list queries over explicit
  states plus create labels and a body marker, then parse metadata to confirm the
  exact key. Provider-side body search is not required for the proof: client-side
  body filtering after state/label narrowing is safe because exact metadata
  confirmation remains mandatory. The parent-aware issue path also ensures a
  found child issue keeps the repo-qualified parent back-reference needed by
  cross-repo fan-out.
- **At-most-once merge.** `MergePullRequest` runs before the label commit point
  and is skipped when the freshly loaded pull request is already merged. A crash
  that lands the merge but loses the response leaves the post-merge labels
  unapplied; the retry observes the merged state, skips the merge, and finishes
  the `landed`/`alignment` projection. Those labels are also the planner
  re-run guard, so the merge happens exactly once and the post-merge projection
  survives on the closed pull request.
- **Applied reconciler actions.** `recover::Applier` applies a `ReconcileReport`
  through the existing components (the executor's idempotent label-apply path,
  `LeaseManager::clear`, and the command journal). Each mutating action loads
  fresh state and applies at most once, so re-running the same report is a
  no-op, not a double-apply. Repairs and unblocks are journaled, so a crash
  between the mutation and the terminal journal update leaves the command
  incomplete for the next scan to re-derive. `Escalate`/`Diagnose` are recorded
  as advisory and never silently mutate workflow state. Running scan→apply to a
  fixpoint therefore converges.
- **Cross-repo dependency reads.** Dependency gates read each target from its own
  repository on every scan. A child repo that is temporarily unreadable records
  a dependency read failure and is treated as not landed, so partial outages
  preserve the block instead of producing a false `Unblock`.

## Limitations discovered by the tests

These are real gaps the robustness tests exposed; they are documented here
rather than hidden:

- **Provider branch-protection policy is not modeled.** The portable gate reads
  the latest CI jobs and requested-reviewer aggregate. Provider-specific rules
  such as required-check configuration, CODEOWNERS, or stale-review dismissal on
  push remain outside the workflow core.
- **Escalation/diagnosis is record-only.** `recover::Applier` applies the
  mutating recovery actions, but `Escalate` and `Diagnose` are advisory: the
  applier records them in `ApplyOutcome::advisory` and performs no Forge
  mutation. Projecting an escalation into a label or comment is left to a
  workflow-specific adapter on top of the advisory list, not decided here.
- **Dependency read failures are not projected to Forge.** A child-repo outage
  is surfaced in `DependencyStatus::read_failures`, but the default reconciler
  report stays clean rather than creating an escalation. Operators that need
  visible outage markers should add a workflow-specific adapter.

## Out of scope

No real LLM or agent-provider integration is exercised. Tests drive the
deterministic in-memory backend through the workflow runtime only.
