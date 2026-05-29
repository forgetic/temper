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
- ☐ **9b — Execute assignee + comment effects.** `Executor::execute` applies
  `SetAssignee`/`RemoveAssignee`/`CreateComment` through the `Forge` trait, with
  idempotency and postconditions. Exercise via the five-role fixture or a
  dedicated execution test.
- ☐ **9c — Execute merge + post-merge labels.** `Executor` applies
  `MergePullRequest` and the post-merge `landed`/`owner-pending` projection.
  Preserve "no merge before required gates" (safety_properties).
- ☐ **10 — PR idempotent create.** `Executor::ensure_pull_request` mirroring
  `ensure_issue`; execute `CreatePullRequest` with a correlation key. Closes a
  limitation in `robustness-guarantees.md`.
- ☐ **11 — External-signal gates (ADR).** New gate class satisfied by a
  Forge-projected condition (e.g. `ci = passed`), not only sibling transitions.
  Remove the zero-role `record_ci_pass`/`record_ci_failure` adapter workaround
  from `reference-delivery`.
- ☐ **12a — First-class relations (ADR).** A `relation` spec primitive
  (parent/dependency/produced-PR) in spec/validated/validation/classification,
  superseding metadata-only relations for the spec.
- ☐ **12b — `dependency_gate` + reconcile unblock.** A relation-driven
  `dependency_gate` that clears `blocked-on-dependency` when prerequisites land,
  plus a reconcile action that applies it mechanically.
- ☐ **13 — Queue activation policy (ADR).** Add `min_depth`/`max_age` to the
  queue primitive and a pure planner predicate so `owner_alignment` services
  cohorts. The ADR covers all queue-primitive extensions (13 and 14).
- ☐ **14 — Queue matching extensions.** Multi-artifact-kind queues and
  disjunctive (OR) label-sets, referencing the Phase 13 ADR. Collapse the
  fixture's split `pr_changes_requested`/`pr_testing_failed` workaround and bind
  `escalations`/`needs_human` to both issues and PRs.

## Adjacent pre-existing limitations (not in this backlog)

These are tracked in `robustness-guarantees.md` and may be scheduled alongside
the phases above but are out of scope for the reference-workflow backlog:

- Compare-and-swap lease acquisition (the claim lease in 9-series leans on it).
- Automatically applying reconciler actions through the executor/lease manager.

## Done definition for the whole backlog

All phases ☑, `reference-delivery.json` carries no expression workarounds, the
reference workflow's full loop (intake → triage → claim → PR → review/test/CI →
merge → post-merge reconcile/alignment) is executable, and
`reference-workflow-gaps.md` is updated to a "resolved" record.
