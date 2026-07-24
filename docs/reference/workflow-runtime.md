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

CI status comes from `list_ci_jobs`, queried with both the PR identity and head
SHA when the backend supplies one. Those filters are conjunctive, and the
runtime independently verifies each returned job's non-empty commit SHA against
the head before aggregation so stale provider results cannot satisfy a gate.
For that current head, jobs are reduced to the latest job per name. The aggregate
is terminal only after every latest job is terminal. `ci_passed` then requires
every conclusion to be explicit success. `ci_failed` is narrower than “red”: it
matches only when at least one latest job explicitly reports the ordinary
`failure` category, which is actionable as a source/build/test repair.
Cancellation, interruption, timeout, runner loss, startup failure,
action-required, neutral/skipped, unknown, and completed jobs without a typed
conclusion instead satisfy `ci_recovery_required`. They remain ineligible to
land, but cannot enter an ordinary writable code-repair queue. A visible terminal
job mixed with a queued or running latest job remains pending.

For a freshly revalidated recovery-required attempt, the engine persists an
exact repository/PR/head/run/attempt/job-set marker before side effects. It
requests one supported provider retry and waits for a newer attempt; uncertain
requests are never repeated. Unsupported, rejected, or exhausted retry may
publish exactly one workflow-configured `pull_request_read_only` diagnostic
assignment with explicit verdicts. No configured diagnostic, or its completion,
converges to one `needs-human` barrier and fingerprint-keyed evidence audit.
Every boundary re-reads the exact current head, and marker/assignment CAS state
makes duplicate webhooks, polling, and daemon replacement idempotent. Recovery
does not derive a verdict from an event, a quiet log, or prior passing output:
only the authoritative latest attempt can become pending, passed, ordinary
failed, or recovery-required. The PR head remains unchanged throughout retry,
diagnosis, and parking; only a later independently observed ordinary failure
may enter the existing writable repair route and produce a repaired head. See
[workflow recovery](workflow-recovery.md#interrupted-current-head-ci-recovery).

Terminal aggregates retain deterministic structured evidence for every latest
job: typed and provider conclusions, bounded provider reason, job/run/attempt
identity, commit SHA, URL, and timestamps. Narrow CI observations, exact-head
monitor transitions, wake coalescing, targeted re-reads, freshness checks, and
completion observability preserve the recovery-required state. Poll/read results
are authoritative; webhook facts only accelerate an exact-target re-read and a
stale or duplicate webhook cannot replace a newer pending, passing, failing,
recovery-required, or changed-head read. A visible recovery-required current-head
job is present CI, not a missing-current-head run.
Review status comes from requested reviewers plus native review events; the
portable review aggregate is not head-SHA-scoped.

## Supported effects and order

Workflow effects are a closed enum so executors and reconcilers must handle every
variant. Implemented transition effects are:

- `AddLabel` / `RemoveLabel`;
- `SetAssignee` / `RemoveAssignee` with role-to-user resolution;
- `CreateComment`;
- `CreatePullRequest` with transition-bound runtime input and optional PR
  artifact-kind metadata;
- `CreateIssues` with transition-bound child issue input, idempotent child keys,
  sibling dependency metadata, optional source-issue dependency recording, and
  child workflow metadata that preserves explicit child `target_branch` values
  while inheriting a non-empty source issue `target_branch` when the child omits
  one;
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

## Runtime-bound plan-validation audits

A successful `validate_plan` result binds a transition-completion audit to that
specific runtime execution. This is not a static workflow `CreateComment`
effect: the verdict applier resolves the authenticated Forge actor before
mutation and builds the comment from the routed outcome, workflow role, exact
job/attempt identity, routed transition, workspace coordination key, and the body-omitted,
already-bounded validation scope. It publishes only the normalized,
secret-redacted, character-bounded `JobResult.summary`; result `body`, `details`,
reasoning, tool output, credentials, and artifact bodies are not audit inputs.
The configured role and authenticated actor are rendered separately, so an
alias such as `tester` running as Forge user `architect` remains explicit.

The ordering is part of the completion contract:

- for `validated`, the landing pull request is ensured first, then the ordinary
  plan comment is ensured, and only then does the plan's final label/assignee
  update commit;
- for `needs_followup`, every child is ensured, numbered, dependency-wired,
  aggregated on the parent, and activated before the comment is rendered with
  final same- or cross-repository child references. The source transition and
  completed create intent commit only after that comment exists.

Each audit carries
`<!-- temper:comment-key=plan-validation:<assignment-key> -->`, where the
assignment key is `assignment-sha256:<digest>`. The digest is derived from a
length-delimited job ID and its optional exact attempt fence. Modern repeated
validation rounds therefore receive distinct records even though their
deterministic job IDs match, while replaying the same exact assignment derives
the same marker and reuses its comment. Legacy unfenced assignments also derive
a deterministic key. The human-facing comment renders the job and attempt IDs
separately. Before append, the runtime lists ordinary comments and skips
creation when the exact marker is already present. This lookup also converges
an uncertain create response without requiring comment edits.

Actor lookup, comment-list, or comment-create failures are reported as
`ConvergencePending`. The lease/result path retains the exact assignment and
worker result, leaves delivery unacknowledged, and retries with backoff; it does
not release the source for a new agent run or recreate already-ensured PRs,
children, dependencies, or comments. For negative validation, the audit
descriptor is persisted in `CreateIssuesCompletion`. Startup recovery completes
the durable child intent through wiring and activation, ensures the marked
ordinary comment, and then commits the source completion while dispatch remains
behind the startup barrier. Recovery therefore does not rerun the tester.

## Idempotency rules

- Re-running a completed label transition fails its label preconditions, so it
  is not double-applied. A `remove_label` effect declared with `"if_present":
  true` is the exception for handoff cleanup: it still verifies the label is
  absent after application but does not require the label to be present before
  planning.
- Assignee set/remove operations are set-like and cleanly idempotent.
- Comments declared by static effects include a hidden marker
  (`<!-- temper:comment-key=<transition>:<comment-index> -->`) and are skipped
  when an existing comment carries the marker. Runtime plan-validation audits
  instead use the exact-assignment-derived `plan-validation:<assignment-key>`
  marker described above.
- `ensure_issue`, `ensure_issue_with_parent`, and `ensure_pull_request` stamp a
  correlation key into workflow metadata before creating, search explicit states
  with bounded summary queries, and parse exact metadata before returning an
  existing artifact. Before a verdict-backed `create_issues` mutates Forge, the
  daemon rejects child kinds with no serviceable queue/action in the active
  workflow. `create_issues` derives one stable key per child, validates sibling
  dependency slugs before mutation, preserves explicit child workflow
  metadata such as `target_branch`, inherits the source issue's non-empty
  `target_branch` for child issues that omit it, creates/ensures every child
  before linking dependencies, and can record the child set on the source issue
  when `record_parent_dependencies: true`; retrying reuses children and appends
  no duplicate dependency refs.
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
labels from the named PR kind, and writes PR metadata containing the kind and
parent links to the source issue followed by any de-duplicated parent refs
already present on the source issue. Missing target-branch metadata aborts before
any labels or assignees are changed.

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
