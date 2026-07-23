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
| A merge is not authorized, and the pull request is not merged, until native review approval and current-head CI gates pass; once gated, it merges and projects `landed`/`alignment` | `a_merge_is_not_authorized_until_review_and_ci_gates_pass` |
| After merge-conflict repair advances a PR head, old green CI cannot authorize landing: absent, queued, and running repaired-head CI all keep the PR open across mechanical ticks; only a later repaired-head success lands it, while preserving the existing approval without a second review request | `reference_conflict_resolution_requires_fresh_ci_but_no_second_review` (`temper-runner/tests/mechanical_merge_conflict.rs`) |
| Mechanical landing does not bypass gates: a PR with `landing` and green CI but no native approval stays open | `reference_landing_pr_without_approval_does_not_merge_even_when_ci_passes` |
| The gate mechanism blocks a merge until native CI conclusions plus native review approval both pass | `the_merge_gate_mechanism_requires_ci_and_review_together`, `ci_gate_reads_native_ci_job_conclusions` |
| A gated merge executes at most once: a crash that lands the merge but loses the response is retried without merging twice | `a_merge_executes_at_most_once_under_retry` |
| Merge rejections are typed only after a re-read: open/unmerged PRs return `MergeConflict`, while a conflict response after a successful merge still projects post-merge labels | `merge_conflict.rs` |
| Mechanical conflict routing preserves `landing`, adds `merge-conflict`, pauses landing automation until PR-targeted repair clears the blocker, and does not block unrelated landable PRs | `mechanical_merge_conflict.rs` |
| A failed review gate returns work to the engineer, and the reviewer cannot perform that return path | `a_failed_review_gate_returns_work_to_the_engineer` |
| Expired in-progress work becomes visible for recovery | `expired_in_progress_work_becomes_visible_for_recovery` |
| Assignment identity and lifecycle projection are committed before publication; startup admits only exact worker/job heartbeat reattachment and rolls back unmatched orphans before dispatch opens | `claim_is_committed_before_assignment_publication`, `matching_heartbeat_reattaches_staged_assignment_and_rejects_other_ids`, `startup_recovery_barrier_defers_enqueue_until_orphans_are_collected` |
| A recovered attempt that loses its exact durable assignment is fenced, recursively quiesced, and represented by one durable canceled result before capacity reopens; blocked/closed removal and newer-attempt replacement are definitive, while a one-shot backend failure remains attached and retries | `hermetic_real_stack/ownership_loss.rs` (`ownership_loss_*`) |
| Standalone SIGINT/SIGTERM uses one absolute internal budget; only fully joined attempts are released, while deadline expiry preserves unproven assignments and trace spool for startup convergence and exits through bounded crash handoff | `temper-cli-daemon::standalone::shutdown::tests`, `temper-cli-daemon::standalone::shutdown::watchdog::tests` |
| Dirty reusable workspaces replay local commits plus tracked/untracked edits over an advanced target, or produce one stable actionable quarantine | `existing_dirty_workspace_replays_local_work_over_advanced_remote`, `conflicting_recovery_is_quarantined_once_with_actionable_manifest` |
| Multi-child creation is durable and staged: restart resumes create/wire/activate without duplicate children or premature dispatch, while a later legitimate execution of the same transition gets a distinct durable round and child correlation identity | `create_intent_recovery.rs`, `repeated_create_rounds.rs`, `staged_children_are_excluded_from_role_scans` |
| Impossible label combinations are detected by both the executor and the reconciler | `impossible_label_combinations_are_detected_not_silently_ignored` |
| Reconciler actions are applied through the runtime and re-applying a report is a no-op | `recovery.rs` (`requeue_lease_clears_the_lease_through_the_manager`, `repair_realizes_pending_labels_and_reconciles_the_command`, `unblock_realizes_labels_and_journals_a_completed_command`, `re_applying_a_report_is_a_no_op`) |
| Cross-repo dependency aggregation unblocks a parent only after every child has landed in its own repository; a transient child read failure is not a false unblock | `dependency_aggregation.rs` |
| Applying a repair/unblock is retry-safe across a crash before or after the write | `crash_injection.rs` (`applying_a_repair_is_retry_safe_under_a_crash`, `applying_an_unblock_is_retry_safe_under_a_crash`) |
| The scan→apply loop converges to a clean state | `recovery.rs` (`the_scan_apply_loop_converges_to_a_clean_state`) |

## Lease acquisition is compare-and-swap

Lease acquisition closes its lost-update window with the portable
optimistic-concurrency primitive (ADR 0013,
`docs/reference/forge-interface-concurrency.md`). `LeaseManager` captures the
artifact's `Version` at load time and writes the
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
  the `landed`/`alignment` projection. A merge `Conflict` response is also
  re-read before routing: already merged continues projection, missing/closed is
  stale, and only still-open unmerged is a typed merge conflict. Mechanical
  conflict fallback preserves `landing` and adds `merge-conflict`; the landing
  queue excludes that blocker label, so normal ticks do not retry the same PR
  until the engineer's PR-targeted repair removes `merge-conflict`. Those labels
  are also the planner re-run guard, so the merge happens exactly once and the
  post-merge projection survives on the closed pull request.
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

## Restart convergence

The hermetic real-stack fixture separates its durable world (MemoryForge state,
local git origins, workspace root, model script, worker identity, result stream,
and mutable wall clock) from replaceable daemon, worker, and executor handles.
Existing workers route through a stable in-process endpoint, so replacing a
daemon exercises reconnect behavior rather than rebuilding test state.
Components expose abrupt stop/join controls, and restart scenarios synchronize at
named one-shot channel barriers: assignment claim commit, worker push, result
application start/completion, child create/wire/activation, and recovery-barrier
opening. Lease time uses the supplied mutable wall clock; engine timers are
advanced through their explicit runtime clock. Correctness assertions therefore
do not infer progress from arbitrary sleeps.

Recovery ordering is: complete durable child intents; inventory assignments with
dispatch closed; accept only exact worker/job heartbeat reattachment; converge
expired, malformed, and unreattached orphans; then open deferred feeds. The
observable idle condition is no dispatchable artifact, live stale lease, staged
intent, duplicate session, or unapplied current-head result.

Dirty checkouts preserve local commits and tracked/untracked edits before moving
to the current target. Successful replay retains the edits; ambiguous replay
creates one stable quarantine manifest with recovery refs and commands. PR
repair similarly converges monotonically: `repaired_head` rejects stale CI and
suppresses repeated repair until current-head CI succeeds.

## Worker liveness, result durability, and live claim convergence

The worker permit owner, not lease renewal, is the authority for agent
progress. Every accepted lifecycle boundary advances a generation-tagged
monotonic deadline. Equality at the deadline rearms for the minimum tick so a
progress/completion stamped exactly at the limit wins; stale timer generations
cannot cancel newer work. On a no-progress or maximum-run timeout the attempt
moves once through `CancelRequested -> Quiesced -> ResultRecorded`, and an
attempt fence blocks late result-file acceptance, validation, push, or duplicate
publication.

Capacity is released only after the transient timeout result is durably recorded
in the restart-readable outbox. Delivery and exact release acknowledgement then
replay independently, so a daemon/Forge outage does not retain the local permit.
A recovered attempt has the same ordering with a stricter authority rule: every
heartbeat must reattach to the exact durable job and attempt. Assignment/lease
removal, a blocked or closed source, and replacement by a newer attempt close the
attempt fence and request cancellation; backend lookup failure remains transient.
Recursive cleanup and endpoint joins precede the sole `Canceled` result and local
capacity release. The daemon keeps its workstream occupied until stale delivery
is acknowledged as `Reclaimed` or `Superseded`, after which the worker compacts
the outbox. Model, tool, submit, Forge-context, workspace, git, push, and ordinary
result effects are rejected after the fence, while the trace forwarder is still
allowed to persist the terminal `cancelled` boundary.

Expired durable assignments are converged from fresh Forge state by the shared
startup/live `AssignmentConverger`; a newer claim is a stale no-op and an outage
leaves the exact assignment available for a later pass. Structured
`worker.result.delivery` and `assignment.convergence` events distinguish pending,
converged, stale, quarantined, and unreconciled outcomes.

The daemon heartbeat/state report is intentionally not part of this proof. It
stores only the latest exact-attempt observation for operators. The authoritative
inputs remain worker monotonic time, the attempt fence/outbox, and durable Forge
assignment/lease metadata.

Deterministic machine tests cover exact-boundary progress, stale timers,
completion-versus-timeout, one durable record, one capacity release, heartbeat
membership, and capacity greater than one
(`temper-worker::worker_machine_watchdog_tests`). Unix supervisor and standalone
runner tests hold cancellation terminal acknowledgement beyond 250 ms and prove
that both carriers preserve the exact `run.finished(status=cancelled)` sequence
without publishing quiescence. The hermetic ownership-loss matrix pauses after
the engine journal becomes terminal but before its acknowledgement reaches the
worker, proving that `AttemptQuiesced`, canceled-result durability, heartbeat
removal, and permit release remain blocked; its restart case replaces the daemon
while terminal forwarding is pending and drains the same spool through the
replacement. Live convergence tests cover Forge outage, retry, idempotence, and
newer-claim fencing (`temper-runner/tests/assignment_convergence.rs`). The
hermetic real-stack ownership-loss matrix additionally runs the real daemon,
worker machine/shell, native agent, MemoryForge, local git, trace spool/journal,
and result outbox through blocked/closed/replaced/transient and restart
boundaries (`temper-testing/tests/hermetic_real_stack/ownership_loss.rs`). On
Linux that focused target explicitly builds an early-main supervisor helper and
injects a forced-supervisor factory into worker-owned fixture commands, retaining
descendant-complete cleanup on clean hosts without delegated cgroup access while
production workers continue to use automatic cgroup/supervisor selection.

## Standalone bounded shutdown is process-loss recovery

Ordinary assignment ownership loss remains proof-based. Closing an attempt
fence suppresses stale model, tool, result, Forge, workspace, Git, and push
effects, but it does not itself prove quiescence. Descendant direct-child reap
and recursive-empty proof plus acknowledgement of the exact terminal trace
sequence remain prerequisites for `AttemptQuiesced`, heartbeat removal, canceled
result durability, and permit release. A blocked proof may therefore wait
indefinitely while the process is otherwise healthy.

`temper serve standalone` has an additional process-level bound:
`deployment.standalone_shutdown_budget_secs` defaults to 30 seconds and is
measured once from signal receipt. Daemon claim/result/context/Forge-application
admission and active attempt fences close before cancellation. Worker join,
already-admitted daemon operations, trace retention, release of exact proven
attempts, and HTTP drain all consume the same deadline; the final five seconds
are reserved for an independent emergency KILL and an immediate core-dump-free
process exit. Split workers retain their ordinary proof-based semantics.

A proven path emits `standalone.shutdown.summary` with
`disposition=graceful_exit`. A deadline blocker emits bounded/redacted
`standalone.shutdown.blocker` events and a final
`disposition=bounded_crash_handoff`; the process does not fabricate cleanup,
terminal-trace acknowledgement, result publication, capacity release, or normal
assignment release. It retains durable assignment metadata and trace spool,
uses attempt-owned out-of-band process termination, and exits with distinct
status 70 without unwinding, C/Rust exit handlers, owner drops, userspace buffer
flushing, or core generation. The replacement startup stages prior-boot
assignments with dispatch closed, converges or requeues abandoned work from
durable Forge state, rejects old attempt results/Forge operations through the
existing fences and exact claim checks, and forwards retained trace records
without reviving the old attempt.

Blocker kinds are `containment`, `terminal_trace_ack`, `result_delivery`,
`component_task`, and `registry_state`. Fields include bounded worker/job/
attempt identity, owner scope/name, root PID and sampled survivor PIDs,
containment phase or trace run/sequence, first-seen time, increasing age,
escalation stage, deadline remaining, and final disposition. Missing process
evidence is reported as unknown rather than converted into proof.

## Limitations discovered by the tests

These are real gaps the robustness tests exposed; they are documented here
rather than hidden:

- **Provider branch-protection policy is not modeled.** The portable gate reads
  the latest CI jobs and requested-reviewer aggregate. Provider-specific rules
  such as required-check configuration, CODEOWNERS, or stale-review dismissal on
  push remain outside the workflow core. Forgejo merge rejection categories are
  also coarse today; an open, unmerged PR after a merge `Conflict` is routed as
  an engineer-visible merge conflict even if the provider rejection was stricter
  policy rather than a textual conflict.
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
