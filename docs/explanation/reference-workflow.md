# Reference delivery workflow

This page designs a clean five-role delivery workflow, using the production
ekanayaka.io agent workflow as inspiration but removing its accidental
complexity. It is the target the harness spec, compiler, planner, and executor
should be able to express and run. The `reference-delivery.json`
`RawWorkflowSpec` fixture transcribes it; any gap between this design and what
the runtime can execute becomes implementation backlog.

This is an explanation page: it defines the conceptual model and the rationale.
The exact spec is the fixture; the exact runtime contract is
`docs/reference/workflow-layer.md`.

## Design principles

The redesign is held to five rules, each chosen so the result maps onto harness
primitives instead of prose:

1. **Every label is serviced by a queue or is a gate condition.** No decorative
   labels.
2. **One label, one meaning.** Generic lifecycle labels are allowed only when
   the meaning stays stable and the workflow declares which artifact kinds may
   carry each state.
3. **Prefer modeling over prose.** If a behavior can be a transition, gate,
   lease, or relation, it must not be a paragraph of prompt instructions.
4. **Queue state is not gate state.** A queue label means "a role must act"; a
   gate label means "a condition holds." Conflating them is the root of most of
   the original workflow's knots.
5. **External concerns stay at the backend/runner boundary.** Git mergeability,
   reading CI, branch protection, and provider-specific review policy are
   projected *into* the workflow as native gate signals or adapter facts — never
   baked into role prose.

## Settled decisions

These were decided during design and are not open:

- **CI and review results are gates fed by native Forge state.** No `needs-ci`
  queue or review-result labels; CI readiness is `ci_gate`, and review approval
  is `review_gate` from the Forge review aggregate.
- **`blocked` is dependency-only.** Dependency blocking is a modeled relation +
  gate (`blocked`, cleared mechanically). Agent escalation is a separate
  `needs-architect` flag routed to the architect. Design feedback uses
  `needs-owner`; the owner may switch it to `needs-human` for explicit human
  judgment.
- **Claims are explicit leases.** The engineer's claim is a `metadata::Lease`
  with a TTL; the reconciler handles expiry. The original prose stale-claim
  recovery procedure is dropped.
- **Effort labels are dropped from the spec.** `easy`/`hard` gated nothing; they
  live as free prompt metadata if useful, not as workflow state.
- **Post-merge has two consumers.** The architect drains landed PRs eagerly and
  per-item (drives dependency unblocking); the owner reviews landed work in
  cohorts via a queue activation policy.

## Roles

| Role | Charter (short) | Primary queues |
| --- | --- | --- |
| `architect` | Turn requests into epics/design/ready code issues; resolve `needs-architect`; reconcile landed PRs. | `design_triage`, `needs_architect`, `landed_inbox` |
| `engineer` | Claim ready code issues, implement, open PRs, address failed review or CI gates. | `code_ready`, `pr_changes_requested`, `pr_ci_failed` |
| `reviewer` | Static review against the contract catalog; approve or request changes. | `pr_needs_review` |
| `owner` | Resolve design feedback, approve gated merges, and run holistic alignment review of landed cohorts. | `needs_owner`, `merge_ready`, `owner_alignment` |
| `human` | Resolve `needs-human` items that require non-agent judgment. | `needs_human` |

## Artifact kinds

- `epic` (issue) — long-lived goal grouping designs and code.
- `design` (issue) — design/refinement work owned by the architect.
- `code` (issue) — an implementation workstream owned by the engineer.
- `implementation_pr` (pull request) — the change produced for a code issue.

Human-filed issues enter as `untriaged` (a positive label, not "absence of a
type label") so intake is a normal queue match. The architect triages them into
`code` or `design`.

## State dimensions

Exclusive unless noted; each state projects to one label.

- `work_lifecycle`: `draft` (legal for `design`), `ready` (legal for
  `design` and `code`), `in_progress` (legal for `epic` and `code`), and
  `blocked` (legal for `code`). The generic labels keep stable meanings; the
  artifact-specific legality matrix prevents combinations such as `code + draft`.
- Review results are not a workflow-owned state dimension; `needs-reviewer` is
  a routing label, while `approved` / `changes_requested` are native Forge
  review decisions read through review gate conditions.
- CI/test status is not a workflow-owned state dimension; merge eligibility and
  failed-CI routing read native CI job conclusions through `ci_passed` and
  `ci_failed` conditions.
- Merge eligibility is derived from gates rather than stored as a state;
  `needs-merge` is only an owner-routing label.
- `attention` (non-exclusive): `needs_architect` (`needs-architect`),
  `needs_owner` (`needs-owner`), `needs_human` (`needs-human`), and
  `needs_merge` (`needs-merge`) route explicit role work.
- `post_merge` (non-exclusive): `landed` (awaiting architect reconcile),
  `alignment` (awaiting owner alignment).

## Gates

- `review_gate` — satisfied by the runtime's native review signal (`review_approved`) derived from requested reviewers and review events.
- `ci_gate` — satisfied by the runtime's native CI signal (`ci_passed`) derived
  from Forge CI job conclusions.
- `dependency_gate` — satisfied when every prerequisite relation of a code issue
  is merged. Drives mechanical unblocking.

The merge transition requires `review_gate` and `ci_gate`. The engineer marks
an implementation PR with `needs-merge`; the `merge_ready` queue wakes the owner
once native CI has passed, and `approve_merge` still rechecks both gates before
mutating. A failed review or CI run returns the PR to the engineer (native
`review_changes_requested` or `ci_failed` → engineer queues); work is never lost.

## Relations

- `design → epic` (parent), `code → design`/`epic` (parent).
- `code → code` (dependency): a code issue is `blocked` until its prerequisite's
  PR lands, then `dependency_gate` clears it to `ready` mechanically.
- `implementation_pr → code` (implements): the PR is produced for a code issue.
  Closing that issue on merge is intended, but the current runner happy path
  accepts it staying open while post-merge PR handling runs.

## Escalation and the human loop

Role-attention labels are requests for another role to act, not verdicts. Any
of engineer or reviewer may set `needs-architect` on a PR or issue;
that routes it to the architect's `needs_architect` queue. The architect clears
that flag after amending a spec, recording a decision, or returning work.

When the architect needs owner feedback on a design issue, they set
`needs-owner`. The owner either clears that flag after responding or switches it
to `needs-human` when non-agent human judgment is required. The explicit
`human` role clears `needs-human` after responding.

## Post-merge handling

Merging an `implementation_pr` fires two independent consumers:

- **Architect, eager and per-item.** A merged PR carries `landed`. The architect
  drains `landed_inbox` as soon as possible: update the epic body, file
  follow-ups, then clear `landed`. Latency matters because a merge may satisfy a
  `dependency_gate`. The dependency unblock itself is mechanical (reconcile/plan
  re-evaluates dependents); the architect handles only the judgment residue.
- **Owner, batched and holistic.** A merged PR also carries `alignment`. The
  owner reviews the cohort and clears the flags; misalignment becomes a normal
  new issue. Clearing the flag is the watermark, so we get auditability (every
  PR was seen) and the cohort window for free.

### Queue activation policy

"Every N landed" is a count over the unreviewed set, not a schedule. Rather than
approximate it with a high cron interval (which under- and over-fires), the
`owner_alignment` queue carries an **activation policy**: service it only when
its depth ≥ `min_depth`, **or** the oldest pending item is older than `max_age`.
The `max_age` companion guarantees liveness on slow weeks. This is a small,
read-side addition to the queue primitive (a spec field plus a pure planner
predicate); it does not touch the executor, leases, journal, or reconciler.

## Properties preserved

The redesign must keep, and can be conformance-tested against
`safety_properties.rs`: design/implementation separation; independent
review + CI gates required to merge; failed gate returns work
(never lost); human-in-the-loop checkpoints; an escalation path for judgment;
at-most-once claiming; no duplicate creates; and no premature merge.

## Anticipated harness additions

The design deliberately leans on capabilities the runtime does not yet execute.
The fixture exercise confirmed and prioritized the exact backlog in
`reference-workflow-gaps.md` (including two queue-primitive gaps this list
missed: multi-artifact-kind and disjunctive queue matching, now resolved). The
expected backlog was:

- **Execute remaining non-label effects** — `CreateComment`, assignee,
  `CreatePullRequest`, and `MergePullRequest` effects now apply through the
  executor (`CreatePullRequest` via idempotent `ensure_pull_request`; merge is
  at-most-once and projects the post-merge `landed`/`alignment` labels).
  Claim-time lease effects remain future work.
- **External-signal gates** (`ci_gate`, `review_gate`) — implemented as gates
  satisfied by runtime signals from native Forge CI jobs and review events, not
  sibling transition labels. `pr_ci_failed` likewise routes from native CI
  status instead of a workflow-owned testing label.
- **Relation-driven dependency handling** — the `relation` primitive declares
  parent/dependency/produced-PR links, native Forge dependency links feed the
  `dependencies_resolved` gate, and the reconciler mechanically produces and
  applies the `blocked` unblock once every prerequisite lands.
- **Queue activation and richer matching** — `min_depth`/`max_age`, multi-kind
  queue targets, disjunctive label-set matching, and queue conditions such as
  `review_changes_requested` are implemented as read-side planner predicates.

The contract catalog and the layered prompt system stay as prose the runner
injects; they are not modeled as harness machinery in this design.
