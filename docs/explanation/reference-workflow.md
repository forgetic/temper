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
| `architect` | Triage intake through the `triage_intake` workspace, create design/code work, resolve architect escalations, reconcile landed PRs. | `design_triage`, `needs_architect`, `landed_inbox` |
| `engineer` | Claim ready code work, implement and open PRs through the coding workspace, address review/CI/conflict returns. | `code_ready`, `pr_changes_requested`, `pr_ci_failed`, `pr_merge_conflict` |
| `reviewer` | Review PRs through the `review_pr` workspace against the contract and the real diff/CI; approve, request changes, or escalate. | `pr_needs_review` |
| `owner` | Resolve design feedback and run holistic alignment review. | `needs_owner`, `owner_alignment` |
| `human` | Resolve explicit non-agent judgment requests. | `needs_human` |
| `mechanical` | Land approved/green PRs and route merge conflicts; not an LLM worker. | automated `landing` queue |

## Artifact kinds

- `epic` issue: long-lived goal grouping designs and code.
- `design` issue: refinement/design work owned by the architect.
- `code` issue: implementation workstream owned by the engineer.
- `implementation_pr` pull request: change produced for a code issue.

`intake` is the default (catch-all) issue kind: it declares no identifying
labels, so a freshly filed human issue with no labels is admitted as a normal
work item rather than left unclassified. The label-less `raw_intake` automation
queue runs the mechanical `mark_untriaged` transition to stamp such an issue
`untriaged`, after which the architect's `design_triage` queue services it. Human
intake is therefore expressed from declaration, without absence-of-type
inference or a provider-specific adapter, and the live mechanical loop drives it
per tick (see [reference-workflow-gaps.md](reference-workflow-gaps.md)).

## State and routing labels

`work_lifecycle` is exclusive: `draft` (design only), `ready` (design/code),
`in_progress` (epic/code/implementation PR), and `blocked` (code).
Artifact-specific legality prevents combinations such as `code + draft`.

Review and CI status are not workflow-owned state dimensions. `needs-reviewer`
routes review work, while native review decisions feed review gates. Transitions
that hand work back to review also clear an `in-progress` label if one is present,
so review routing does not leave a stale working-state signal behind. Native CI
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
adds `merge-conflict` while preserving `landing`; the landing queue excludes the
conflict label, so mechanical retries pause and a PR-targeted engineer repair
workspace rebases or merges main, resolves the conflict, and pushes a new head.
Fresh CI is still required before landing resumes.

## Relations and dependencies

- `design -> epic` and `code -> design` / `epic` are parent relations.
- `code -> code` is a dependency relation: blocked code unblocks mechanically
  when every prerequisite has landed.
- `implementation_pr -> code` is the produced-PR relation.

Closing the produced code issue on PR merge is not automatic in the engine. The
demo/dependency scenarios use architect-side landed reconciliation to close the
code issue and clear `in-progress`, making native issue-closed state observable
to dependency gates.

## Workspace-backed roles

Three roles act through a sandboxed workspace (ADR 0022): the workspace analyses
with granted tools, produces a work product, and returns a verdict that the
engine routes to a transition through the action's `outcomes` map. The engine
treats verdict ids as opaque vocabulary and owns transition legality and effect
application; the workspace never mutates Forge.

- **Architect `triage_intake`** routes `ready_code` to `triage_intake_to_code`
  (rewrite the body with `set_body` into a crisp code spec, then `code` +
  `ready`), `needs_design` to `triage_intake_to_design` (author a design proposal
  body, then `design` + `needs-owner`), and `needs_breakdown` to
  `triage_intake_breakdown` (`create_issues` for dependent children, with the
  parent kept as an epic plan record). Architect-owned triage/follow-up work
  assigns the item to the `architect` role as the declarative pickup signal.
- **Engineer `open_pr`** opens a PR from the head the coding workspace produces,
  routes the `needs_architect` verdict to `request_code_architect_input` when a
  ready code issue is underspecified or unimplementable as written, or routes
  `needs_human` to `request_code_human_input` only when implementation needs
  non-agent judgment. Both decline paths escalate instead of looping on an empty
  diff.
- **Reviewer `review_pr`** reads the real diff and CI and routes `approve` to
  `approve_review`, `changes` to `request_changes_with_review` (a native
  `attach_review` carrying the authored review body), and `escalate` to
  `request_architect_input`.

These declarations replace the earlier prompt workarounds — the reviewer no
longer approves from the PR summary, and the engineer no longer relies on prose
to explain that `open_pr` is what produces the diff.

## Escalation and human loop

Attention labels request another role; they are not verdicts. Engineers or
reviewers can set `needs-architect`; the architect clears it after amending a
spec, recording a decision, or returning work. Engineers set `needs-human` only
for explicit non-agent judgment on code work. If owner feedback is needed, the
architect sets `needs-owner`; the owner either clears it or switches to
`needs-human` for explicit non-agent judgment. The human role clears
`needs-human` after resolution, returning code work to `ready` when applicable.

## Post-merge handling

Current mechanical landing creates two independent consumers:

- The architect drains `landed` eagerly and per item to update epics, file
  follow-ups, and in demo variants close the produced code issue. Low latency
  matters because the close may satisfy dependency gates.
- The owner drains `alignment` in cohorts. `owner_alignment` activates when the
  pending count reaches `min_depth` or the oldest item exceeds `max_age`, giving
  batching without losing liveness.

A planned workflow-native validator role is a third, separate post-merge
handoff, not a replacement for those queues. The workflow spec decides whether
that handoff validates each merged implementation PR or an aggregate target such
as an epic after child completion or an explicit validation-ready signal. Its
binding model plus context and result schemas are defined in
[post-merge-validator-handoff.md](../reference/post-merge-validator-handoff.md);
the current `temper-scenario validate-pr` command is only a temporary/manual
bridge for producing validation reports.

## Properties preserved

The workflow is intended to preserve design/implementation separation,
independent review and CI gates, failed-gate return paths, human checkpoints,
judgment escalation, at-most-once claiming, no duplicate creates, no premature
merge, and non-blocking mechanical landing. These are covered by
`crates/temper-workflow/tests/safety_properties.rs` and related robustness tests.
