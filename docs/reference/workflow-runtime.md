# Workflow runtime execution

This page defines how a planned transition becomes Forge mutation. For pure
planning, read [workflow-classification-planning.md](workflow-classification-planning.md).

## Executor loop

`execute::Executor` is generic over `F: Forge + ?Sized`; it owns no durable state
and can run against a concrete backend or `&dyn Forge`. Every mutating execution:

1. loads fresh Forge state for the target issue or pull request;
2. classifies it under the validated workflow;
3. reads only the gate signal families the transition needs;
4. re-plans against that fresh state and those signals;
5. applies supported effects idempotently through the `Forge` trait;
6. verifies postconditions or returns typed diagnostics.

The executor never trusts a stale plan. It refuses to mutate on validation,
classification, precondition, gate, impossible-state, unsupported-effect,
unresolved-role, missing-correlation-key, or missing-PR-input failures.

## Gate signal reads

`Executor::read_gate_signals` exposes the full read-only load/classify/signal
path. `read_gate_signals_with_needs` and
`read_classified_gate_signals_with_needs` are the lazy scan variants used after
cheap queue matching.

Dependency status is derived by reading each target in its own repository: issues
are landed when closed; pull requests are landed when merged. A temporarily
unreadable target is recorded as a read failure and remains not landed. When a
reference backend has an issue and pull request with the same number, dependency
resolution treats the issue as authoritative.

CI status comes from `list_ci_jobs`, scoped to the PR head SHA when the backend
supplies one. Review status comes from requested reviewers plus native review
events; the portable review aggregate is not head-SHA-scoped.

## Supported effects and order

Workflow effects are a closed enum so executors and reconcilers must handle every
variant. Implemented transition effects are:

- `AddLabel` / `RemoveLabel`;
- `SetAssignee` / `RemoveAssignee` with role-to-user resolution;
- `CreateComment`;
- `CreatePullRequest` with transition-bound runtime input and optional PR
  artifact-kind metadata;
- `CreateIssues` with transition-bound child issue input, idempotent child keys,
  sibling dependency metadata, and optional source-issue dependency recording;
- `RequestReviewers` with role-to-user resolution;
- `SubmitReview` (`approved`, `changes_requested`, `commented`, or `pending`,
  though Forgejo rejects pending submission);
- `MergePullRequest`.

`UpdateLease` and `ReleaseLease` remain placeholders and are rejected by the
executor; lease mutation goes through `LeaseManager`.

Effect application order is intentional: idempotent comments first, PR creation
next, child issue creation and sibling/parent dependency linking, reviewer
requests and review submissions, PR merge, then labels and assignees together in
one backend update. Creates and merges therefore happen before the label commit
point, while retries can still finish the state projection.

## Idempotency rules

- Re-running a completed label transition fails its label preconditions, so it
  is not double-applied. A `remove_label` effect declared with `"if_present":
  true` is the exception for handoff cleanup: it still verifies the label is
  absent after application but does not require the label to be present before
  planning.
- Assignee set/remove operations are set-like and cleanly idempotent.
- Comments include a hidden marker
  (`<!-- temper:comment-key=<transition>:<comment-index> -->`) and are skipped
  when an existing comment carries the marker.
- `ensure_issue`, `ensure_issue_with_parent`, and `ensure_pull_request` stamp a
  correlation key into workflow metadata before creating, search explicit states
  with bounded summary queries, and parse exact metadata before returning an
  existing artifact. `create_issues` derives one stable key per child, validates
  sibling dependency slugs before mutation, creates/ensures every child before
  linking dependencies, and can record the child set on the source issue when
  `record_parent_dependencies: true`; retrying reuses children and appends no
  duplicate dependency refs.
- `MergePullRequest` is skipped when the freshly loaded PR is already merged. A
  merge `Conflict` is re-read: already merged continues post-merge projection,
  missing/closed is stale, and open/unmerged becomes `MergeConflict`.

Postconditions are checked against the artifact returned by the commit update,
not a later reload, so a concurrent worker cannot make a successful transition
look failed by advancing the artifact after the commit.

## PR creation and merge projection

`CreatePullRequest` carries an optional correlation key and an optional
`artifact_kind` in the spec. Branch, title, body, labels, assignees, and a
runtime correlation key still come from `ExecutionContext`; a missing effective
key or missing create input fails before mutation. When `artifact_kind` is set,
validation guarantees it names a pull-request artifact kind. The executor uses
that kind's stable identifying labels for correlation lookup while callers use
the kind's identifying plus initial labels for creation.

On the daemon verdict path, an issue transition with
`create_pull_request.artifact_kind` is metadata-driven: before executing the
transition, the daemon binds the PR source branch from the source issue's
`WorkflowMetadata.target_branch`, targets the repository default branch, derives
labels from the named PR kind, and writes PR metadata containing the kind and a
parent link to the source issue. Missing target-branch metadata aborts before any
labels or assignees are changed.

Post-merge `landed` and `alignment` labels are ordinary `add_label` effects on
the merge transition. They survive on the closed PR and act as the planner
re-run guard: once present, the merge transition no longer satisfies its add-label
preconditions.

## RoleTools boundary

Agents operating under workflow control must not mutate Forge state directly.
Generated workflow tools are the transition boundary. Runner `RoleTools` exposes
narrow capabilities that still route through workflow/runtime contracts, such as
`ensure_issue_in_repo` for cross-repo fan-out and `close_issue` for the
reference workflow's post-merge issue close.
