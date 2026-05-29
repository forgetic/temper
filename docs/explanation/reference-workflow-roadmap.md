# Reference workflow: implementation roadmap

This roadmap sequences the gap backlog in `reference-workflow-gaps.md` into
single-session work items. It exists so an agent loop can drive the work to
completion one session at a time. Each phase is sized to land green
(`cargo fmt --all`, `cargo dev-clippy`, `cargo dev-check`, crate tests) with its
docs updated and a commit pushed.

The prompts that drive each session live outside the repo (handed to the agent
loop). This doc is the durable record of the plan and progress.

## Conventions for every phase

- `reference-delivery.json` is the **evolving target** fixture. Extend it as new
  primitives land; its planning test is `tests/reference_delivery.rs`.
- `five-role-delivery.json` is the **stable** executor/safety fixture. Only touch
  it when an execution capability it can exercise actually lands.
- Primitive-extending phases write an ADR first (flagged below). These ADRs
  encode design decisions the human flagged during scoping; the roadmap's
  approval greenlights drafting them.
- Keep docs ≤150 lines (split before 350) and Rust files ≤600 lines.

## Phases

Status legend: ☐ pending · ☑ done.

- ☑ **9a — Non-label effect expression.** Extend `RawEffect`/`Effect`/
  `plan::WorkflowEffect` so transitions can express `SetAssignee`,
  `RemoveAssignee`, `CreateComment`, `CreatePullRequest`, `MergePullRequest`.
  Wire validation and planner emission. Make `reference-delivery` use them
  (`open_pr` creates a PR, `approve_merge` merges, `claim_code` sets assignee).
  No execution yet; `Executor` still rejects them with `UnsupportedEffect`.
- ☑ **9b — Execute assignee + comment effects.** `Executor::execute` applies
  `SetAssignee`/`RemoveAssignee`/`CreateComment` through the `Forge` trait, with
  role-to-user resolution, comment idempotency markers, and assignee/label
  postconditions. Exercised by a dedicated execution test.
- ☑ **9c — Execute merge + post-merge labels.** `Executor::execute` applies
  `MergePullRequest` through the Forge merge API at most once (an already-merged
  target is skipped) before the label commit point, and projects the post-merge
  `landed`/`owner-pending` labels as ordinary `add_label` effects on the merge
  transition. `safety_properties` proves no premature merge, at-most-once merge
  under crash/retry, and the post-merge projection.
- ☑ **10 — PR idempotent create.** `Executor::ensure_pull_request` mirrors
  `ensure_issue`; `Executor::execute` runs `CreatePullRequest` through that path
  with a correlation key and runtime create input. Closes the prior
  PR-create limitation in `robustness-guarantees.md`.
- ☑ **11 — External-signal gates (ADR).** New gate class satisfied by a
  Forge-projected condition, not only sibling transitions. Removed the zero-role
  CI adapter-transition workaround from `reference-delivery`; native CI later
  replaced the projected CI labels in ADR 0014.
- ☑ **12a — First-class relations (ADR).** A `relation` spec primitive
  (parent/dependency/produced-PR) in spec/validated/validation/classification,
  superseding metadata-only relations for the spec.
- ☑ **12b — `dependency_gate` + reconcile unblock.** Added the
  `dependencies_resolved` gate condition and a relation-driven `dependency_gate`
  (gating `mark_code_ready`). `Planner::dependency_unblocks` plus a reconciler
  `DependenciesResolved` finding / `Unblock` action clear `blocked`
  mechanically once every prerequisite lands; ADR 0015 later made landed status
  derive from native Forge dependency targets. No new ADR here (extends existing
  relation and gate primitives).
- ☑ **13 — Queue activation policy (ADR).** Added `min_depth`/`max_age` to the
  queue primitive and a pure planner predicate so `owner_alignment` services
  cohorts. ADR 0012 covers all queue-primitive extensions (13 and 14).
- ☑ **14 — Queue matching extensions.** Multi-artifact-kind queues and
  disjunctive (OR) label-sets, referencing ADR 0012. Collapsed the
  fixture's split `pr_changes_requested`/`pr_testing_failed` workaround and bound
  `escalations`/`needs_human` to both issue and PR artifact kinds.

## Adjacent pre-existing limitations (not in this backlog)

These are tracked in `robustness-guarantees.md` and may be scheduled alongside
the phases above but are out of scope for the reference-workflow backlog:

- Compare-and-swap lease acquisition (the claim lease in 9-series leans on it).
- Automatically applying reconciler actions through the executor/lease manager.

## Done definition for the whole backlog

All phases ☑, `reference-delivery.json` carries no expression workarounds, the
reference workflow's full loop (intake → triage → claim → PR → review/test/CI →
merge → post-merge reconcile/alignment) is expressible/plannable, and
`reference-workflow-gaps.md` is updated to a "resolved" record.

**Status: complete.** Phases 9a–14 have all landed. The fixture carries no
expression workarounds and the full loop validates, compiles, and plans. The
only residual items are the adjacent pre-existing limitations below
(compare-and-swap leases; automatically *applying* reconciler actions, including
the dependency `Unblock`), which were always out of scope for this backlog and
are tracked in `docs/reference/robustness-guarantees.md`.
