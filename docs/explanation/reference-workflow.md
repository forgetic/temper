# Reference delivery workflow

This page explains the reference delivery workflow: five judgment roles plus one
mechanical authority for landing. The exact runnable spec is
`crates/temper-workflow/fixtures/reference-delivery.json`; the runtime contract
is [workflow-layer.md](../reference/workflow-layer.md). Gaps and sequencing live
in [reference-workflow-gaps.md](reference-workflow-gaps.md) and
[reference-workflow-roadmap.md](reference-workflow-roadmap.md).

## Design principles

1. Every label is serviced by a queue or is a gate condition.
2. One label has one meaning; generic labels are allowed only when artifact
   legality keeps their meaning stable.
3. Prefer modeled transitions, gates, leases, and relations over prompt prose.
4. Queue state is not gate state.
5. Backend facts such as CI, review state, mergeability, and dependencies enter
   through native signals or adapter facts, not role instructions.

## Settled decisions

- CI and review results are native Forge gate signals. No workflow-owned review
  result labels or CI status labels are used.
- `blocked` means dependency-blocked only. Judgment escalation uses
  `needs-architect`, `needs-owner`, or `needs-human`.
- Claims are metadata leases with TTL-based recovery.
- Effort labels (`easy`/`hard`) are out of workflow state because they gate no
  behavior.
- Mechanical landing projects post-merge `landed` and `alignment`; architects
  drain landed PRs per item, owners review alignment in cohorts.
- The landing queue is not FIFO. A conflicted PR routes to the engineer instead
  of blocking unrelated landable PRs.

## Roles

| Role | Purpose | Primary queues |
| --- | --- | --- |
| `architect` | Triage requests, create design/code work, resolve architect escalations, reconcile landed PRs. | `design_triage`, `needs_architect`, `landed_inbox` |
| `engineer` | Claim ready code work, implement, open PRs, address review/CI/conflict returns. | `code_ready`, `pr_changes_requested`, `pr_ci_failed`, `pr_merge_conflict` |
| `reviewer` | Review PRs against the contract and approve or request changes. | `pr_needs_review` |
| `owner` | Resolve design feedback and run holistic alignment review. | `needs_owner`, `owner_alignment` |
| `human` | Resolve explicit non-agent judgment requests. | `needs_human` |
| `mechanical` | Land approved/green PRs and route merge conflicts; not an LLM worker. | automated `landing` queue |

## Artifact kinds

- `epic` issue: long-lived goal grouping designs and code.
- `design` issue: refinement/design work owned by the architect.
- `code` issue: implementation workstream owned by the engineer.
- `implementation_pr` pull request: change produced for a code issue.

Human-filed issues enter with an explicit `untriaged` label so intake is a normal
queue match rather than absence-of-type inference.

## State and routing labels

`work_lifecycle` is exclusive: `draft` (design only), `ready` (design/code),
`in_progress` (epic/code), and `blocked` (code). Artifact-specific legality
prevents combinations such as `code + draft`.

Review and CI status are not workflow-owned state dimensions. `needs-reviewer`
routes review work, while native review decisions feed review gates. Native CI
job conclusions feed `ci_passed` / `ci_failed` conditions. Merge eligibility is
derived from review + CI gates; `landing` means approval has queued mechanical
landing, not that merge gates are currently satisfied.

Non-exclusive attention labels are `needs-architect`, `needs-owner`,
`needs-human`, `landing`, and `merge-conflict`. Non-exclusive post-merge labels
are `landed` (architect reconcile) and `alignment` (owner review).

## Gates and failure routes

- `review_gate`: native `review_approved` from requested reviewers and review
  events.
- `ci_gate`: native `ci_passed` from Forge CI job conclusions, scoped to the PR
  head SHA when the backend supplies one.
- `dependency_gate`: all prerequisite `dependency` relations have landed.

`land_pr` requires review and CI gates and rechecks both immediately before
merging. A reviewer approval removes `needs-reviewer` and adds `landing`; the
automated queue only services PRs whose native CI has passed. Failed review or
CI routes back to the engineer. CI failure after landing approval clears
`landing` before requesting review so it cannot bypass review. Merge conflict
removes `landing`, adds `merge-conflict`, and returns to the engineer; the
engineer can requeue landing after producing a new head, and fresh CI is still
required.

## Relations and dependencies

- `design -> epic` and `code -> design` / `epic` are parent relations.
- `code -> code` is a dependency relation: blocked code unblocks mechanically
  when every prerequisite has landed.
- `implementation_pr -> code` is the produced-PR relation.

Closing the produced code issue on PR merge is not automatic in the engine. The
demo/dependency scenarios use architect-side landed reconciliation to close the
code issue and clear `in-progress`, making native issue-closed state observable
to dependency gates.

## Escalation and human loop

Attention labels request another role; they are not verdicts. Engineers or
reviewers can set `needs-architect`; the architect clears it after amending a
spec, recording a decision, or returning work. If owner feedback is needed, the
architect sets `needs-owner`; the owner either clears it or switches to
`needs-human` for explicit non-agent judgment. The human role clears
`needs-human` after resolution.

## Post-merge handling

Mechanical landing creates two independent consumers:

- The architect drains `landed` eagerly and per item to update epics, file
  follow-ups, and in demo variants close the produced code issue. Low latency
  matters because the close may satisfy dependency gates.
- The owner drains `alignment` in cohorts. `owner_alignment` activates when the
  pending count reaches `min_depth` or the oldest item exceeds `max_age`, giving
  batching without losing liveness.

## Properties preserved

The workflow is intended to preserve design/implementation separation,
independent review and CI gates, failed-gate return paths, human checkpoints,
judgment escalation, at-most-once claiming, no duplicate creates, no premature
merge, and non-blocking mechanical landing. These are covered by
`crates/temper-workflow/tests/safety_properties.rs` and related robustness tests.
