# Reference delivery workflow

This page designs a clean five-role delivery workflow, using the production
ekanayaka.io agent workflow as inspiration but removing its accidental
complexity. It is the target the harness spec, compiler, planner, and executor
should be able to express and run. A later `RawWorkflowSpec` fixture will
transcribe it; the gap between this design and what the runtime can execute
becomes the prioritized implementation backlog.

This is an explanation page: it defines the conceptual model and the rationale.
The exact spec is the fixture; the exact runtime contract is
`docs/reference/workflow-layer.md`.

## Design principles

The redesign is held to five rules, each chosen so the result maps onto harness
primitives instead of prose:

1. **Every label is serviced by a queue or is a gate condition.** No decorative
   labels.
2. **One label, one meaning.** No overloaded states.
3. **Prefer modeling over prose.** If a behavior can be a transition, gate,
   lease, or relation, it must not be a paragraph of prompt instructions.
4. **Queue state is not gate state.** A queue label means "a role must act"; a
   gate label means "a condition holds." Conflating them is the root of most of
   the original workflow's knots.
5. **External concerns stay at the backend/runner boundary.** Git mergeability,
   reading CI, and branch protection are projected *into* the workflow as gate
   conditions or labels by an adapter — never baked into role prose.

## Settled decisions

These were decided during design and are not open:

- **CI is a gate fed by an external signal, not a queue label.** No `needs-ci`
  queue; CI readiness is `ci_gate`, projected from the Forge CI signal.
- **`blocked` is split.** Dependency blocking is a modeled relation + gate
  (`blocked_on_dependency`, cleared mechanically). Judgment escalation is a
  separate `escalated` flag routed to the architect. Human input is `needs_human`
  routed to the owner.
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
| `architect` | Turn requests into epics/design/ready code issues; resolve escalations; reconcile landed PRs. | `design_triage`, `escalations`, `landed_inbox` |
| `engineer` | Claim ready code issues, implement, open PRs, address failed gates. | `code_ready`, `pr_changes_requested` |
| `reviewer` | Static review against the contract catalog; approve or request changes. | `pr_needs_review` |
| `tester` | Exercise the change; record testing passed or failed. | `pr_needs_testing` |
| `owner` | Resolve `needs_human` items; holistic alignment review of landed cohorts. | `needs_human`, `owner_alignment` |

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

- `design_lifecycle`: `draft`, `ready`.
- `code_lifecycle`: `ready`, `in_progress`, `blocked_on_dependency`.
- `review`: `needs_review`, `approved`, `changes_requested`.
- `testing`: `needs_testing`, `passed`, `failed`.
- `ci`: `pending`, `passed`, `failed` — projected by an adapter from the Forge
  CI signal, not written by an agent role.
- `merge`: `merge_ready` (single state).
- `escalation` (non-exclusive): `escalated` — judgment escalation flag.
- `human` (non-exclusive): `needs_human` — owner-input flag.
- `post_merge` (non-exclusive): `landed` (awaiting architect reconcile),
  `owner_pending` (awaiting owner alignment).

## Gates

- `review_gate` — satisfied by the reviewer's approve transition.
- `testing_gate` — satisfied by the tester's pass transition.
- `ci_gate` — satisfied by the external CI signal (`ci = passed`) using the
  external-signal gate class.
- `dependency_gate` — satisfied when every prerequisite relation of a code issue
  is merged. Drives mechanical unblocking.

The merge transition requires `review_gate`, `testing_gate`, and `ci_gate`. A
failed review or testing gate returns the PR to the engineer (`changes_requested`
/ `failed` → engineer's `pr_changes_requested` queue); work is never lost.

## Relations

- `design → epic` (parent), `code → design`/`epic` (parent).
- `code → code` (dependency): a code issue is `blocked_on_dependency` until its
  prerequisite's PR lands, then `dependency_gate` clears it to `ready`
  mechanically.
- `implementation_pr → code` (implements/closes): merging the PR closes the
  issue and fires post-merge handling.

## Escalation and the human loop

Escalation is for judgment beyond the contract catalog. Any of engineer,
reviewer, or tester may set `escalated` on a PR or issue; that routes it to the
architect's `escalations` queue. The architect resolves by amending a spec,
recording a decision, returning work to the engineer, or — when human input is
required — setting `needs_human`, which routes to the owner. The owner's response
clears `needs_human` (an explicit signal, not a "latest comment isn't mine"
predicate).

## Post-merge handling

Merging an `implementation_pr` fires two independent consumers:

- **Architect, eager and per-item.** A merged PR carries `landed`. The architect
  drains `landed_inbox` as soon as possible: update the epic body, file
  follow-ups, then clear `landed`. Latency matters because a merge may satisfy a
  `dependency_gate`. The dependency unblock itself is mechanical (reconcile/plan
  re-evaluates dependents); the architect handles only the judgment residue.
- **Owner, batched and holistic.** A merged PR also carries `owner_pending`. The
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
review + testing + CI gates all required to merge; failed gate returns work
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
  at-most-once and projects the post-merge `landed`/`owner-pending` labels).
  Claim-time lease effects remain future work.
- **External-signal gates** (`ci_gate`) — implemented as gates satisfied by a
  Forge-projected label/state condition, not only a sibling transition's labels.
- **Relation-driven dependency handling** — the `relation` primitive now
  declares parent/dependency/produced-PR links; `dependency_gate` and the
  reconcile action for mechanical unblocking remain future work.
- **Queue activation and richer matching** — `min_depth`/`max_age`, multi-kind
  queue targets, and disjunctive label-set matching are implemented as read-side
  planner predicates.

The contract catalog and the layered prompt system stay as prose the runner
injects; they are not modeled as harness machinery in this design.
