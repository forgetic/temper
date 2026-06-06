# Reference workflow: gap record

This page records the expression and runtime gaps discovered while transcribing
`reference-workflow.md` into `crates/temper-workflow/fixtures/reference-delivery.json`
and exercising it through `tests/reference_delivery.rs`.

The label-state-machine core — roles, type/state labels, artifact-scoped state
legality, exclusive and non-exclusive dimensions, queues with activation and
richer matching, role-authorized label transitions, transition-satisfied gates,
external-signal gates, relation declarations, the relation-driven
`dependency_gate`, non-label
effects for assignees/comments/PR create/reviewer requests/review submissions/merge,
and native dependency links plus native reviews — **validates, compiles, and
plans today**.

## Resolved expression gaps

- **Non-label effect expression/execution.** Assignee, comment, pull-request
  create, and pull-request merge effects are modeled and the executor applies
  the supported effects idempotently.
- **Pull-request idempotent create.** `CreatePullRequest` runs through
  `Executor::ensure_pull_request` with a correlation key and runtime create
  input.
- **External-signal gates.** `ci_gate` now uses the `ci_passed` condition fed by
  native Forge CI jobs, `pr_ci_failed` uses `ci_failed`, and `review_gate` uses
  `review_approved` fed by native pull-request reviews; the old
  adapter-projected CI/testing and review result labels/state are retired.
- **Relation primitive.** The spec declares `parent`, `dependency`, and
  `produced_pr` relations; classifiers type native dependency links and metadata
  projections using those declarations.
- **Relation-driven dependency unblocking.** The `dependencies_resolved` gate
  condition powers a `dependency_gate`; `Planner::dependency_unblocks` and a
  reconciler `DependenciesResolved` finding / `Unblock` action clear
  `blocked` mechanically once every prerequisite has landed. The
  runtime derives landed status from native Forge dependency targets (closed
  issues or merged pull requests), like the CI signal.
- **Applying reconciler actions.** `recover::Applier` applies a
  `ReconcileReport` through the existing runtime components: lease clears go
  through `LeaseManager::clear`, label repairs and the mechanical unblock reuse
  the executor's idempotent label-apply path, and journal transitions go through
  the command journal. Applying is idempotent and crash-safe, and the scan→apply
  loop converges.
- **Queue activation and mechanical landing.** `RawQueue`/`ValidatedQueue` carry
  optional `min_depth`/`max_age`; `owner_alignment` uses the planner's pure
  activation predicate for cohorts. Queue automation metadata drives the
  reference `landing` queue through the mechanical worker, with merge-conflict
  fallback routing to the engineer.
- **Multi-artifact-kind queues.** A queue can now select several artifact kinds.
  `needs_architect` covers issue artifact kinds and `implementation_pr` without
  duplicate per-kind queues; `needs_owner`/`needs_human` cover the design-issue
  feedback handoff.
- **Disjunctive queue label-sets and native queue conditions.** Queue `labels`
  remain an AND filter and `any_of` adds OR branches. The review return queue
  keys off native `review_changes_requested`; failed CI routing keys off native
  `ci_failed` rather than a label.

ADR 0012 covers the queue primitive extensions as one decision: Phase 13's
activation policy plus Phase 14's multi-kind and disjunctive matching.

- **Workspace-backed intake-to-merge flow (ADR 0022).** The reference workflow
  now expresses the full intake-to-merge flow from declaration alone — it
  validates, compiles, and plans. A default (catch-all) `intake` issue kind
  admits raw human issues that carry no labels (the default-kind support from
  #35); a mechanical `mark_untriaged` transition stamps them `untriaged`; the
  architect `triage_intake` workspace action routes its verdict — `ready_code`
  rewrites the body (`set_body`) into a ready code issue, `needs_design` rewrites
  it into a design proposal headed for the owner, and `needs_breakdown` plans
  dependent children (`create_issues`) with the parent kept as an epic plan
  record. The engineer `open_pr` workspace routes `needs_architect` to escalation
  instead of looping on an empty diff, and the reviewer `review_pr` workspace
  reads the real diff and CI to route `approve` / `changes` (with a native
  `attach_review` carrying the authored body) / `escalate`. The prompt-level
  workarounds — the reviewer told to approve from the PR summary and the engineer
  told that `open_pr` is what produces the diff — are gone, because the verdict
  carries the judgment and the workspace produces the work product. This resolves
  the capstone of #27 (ADR 0022) and folds in the default-kind classification and
  validation support from #35.
- **Live mechanical servicing of default-kind intake (#35).** The reference
  workflow declares a label-less `raw_intake` automation queue that selects the
  default `intake` kind and runs the mechanical `mark_untriaged` transition, so a
  freshly filed unlabeled human issue is stamped `untriaged` by the live
  mechanical loop and then picked up by the architect's `design_triage` queue —
  no seeded-intake label or out-of-band `mark_untriaged` invocation required. The
  bounded-scan policy is extended by exactly one deliberate exception: discovering
  raw intake costs a single state-bounded open-all issue listing per tick (open +
  summary, never closed history). Re-running the stamp on an already-`untriaged`
  issue is a `ContradictedPrecondition` no-op (`unchanged`, no churn), and once
  triaged the issue classifies as `code`/`design`/`epic` and leaves the queue.
  Covered by `reference_delivery::reference_fixture_validates_with_expected_shape`,
  `scan_candidate_planning::automated_reference_plan_permits_single_default_kind_open_all_issue_query`,
  and the `mechanical_worker`/`filesystem_hints` bounded-scan assertions.

## Remaining limitations

- **Claim-time lease effects.** Claims can be represented with
  `metadata::Lease` and managed through `LeaseManager`, but transition specs do
  not yet emit lease effects inside `Executor::execute`.
- **Escalation/diagnosis projection.** `recover::Applier` applies the mutating
  recovery actions (including the mechanical `Unblock`), but `Escalate` and
  `Diagnose` stay advisory: projecting them into a label or comment is left to a
  workflow-specific adapter rather than being decided in the runtime layer.
- **Provider merge-rejection precision.** The workflow can route typed merge
  conflicts, but Forgejo currently exposes coarse merge rejections. Temper
  conservatively routes an open/unmerged PR to `merge-conflict` after such a
  rejection until the backend can distinguish textual conflicts from policy
  failures.
