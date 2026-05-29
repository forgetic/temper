# ADR 0016: Model native pull-request reviews portably

## Status

Accepted

## Context

Native-Forge-state Phase C retires workflow-owned review result labels. Review
requests and decisions are native pull-request state in both target providers,
but each provider also has richer policy: CODEOWNERS, branch protection,
dismiss-stale-reviews-on-push, and review threads. The workflow needs only the
small common subset that can drive gates without leaking provider rules.

## Decision

Add a minimal review model to `harness-forge`:

- pull requests carry `requested_reviewers: Vec<UserId>`;
- `request_pull_request_reviewers` adds reviewers set-like and idempotently;
- `list_pull_request_reviews` returns deterministic review events;
- `submit_pull_request_review` records a review by the backend client's current
  user;
- each review has a typed `ReviewId`, reviewer, decision (`approved`,
  `changes_requested`, `commented`, or `pending`), optional body, and timestamp.

The portable aggregate is deliberately simple: the latest non-comment review per
reviewer wins (timestamp, then stable review id as a tie-break). A pull request
is approved only when at least one reviewer is requested, every requested
reviewer's latest non-comment decision is `approved`, and no reviewer's latest
non-comment decision is `changes_requested`. `commented` does not affect the
aggregate; `pending` blocks approval without counting as changes requested.

Provider-specific rules are outside the portable contract. CODEOWNERS, required
reviewer policies, stale-review dismissal on push, and review threads remain
provider features. Backends that lack one of those features degrade by exposing
only requested reviewers and submitted review events; workflow policy can still
request reviewers and gate on the portable aggregate.

Requesting reviewers is modeled as a workflow transition effect,
`request_reviewers`, whose payload is workflow role ids. The executor resolves
those roles to Forge users through `ExecutionContext`, matching assignee effects.
This keeps the planner pure and keeps reviewer selection explicit in the
workflow spec rather than hidden in executor policy. Reviewer decisions are
modeled as native review submission effects for the reference workflow's
`approve_review` and `request_changes` transitions; real provider adapters can
also observe reviews submitted through the provider UI.

## Consequences

- The review gate reads a runtime review signal derived from fresh Forge state,
  not `review-approved` labels.
- The reference backends persist requested reviewers on pull-request records and
  review events beside pull-request comments, with deterministic identifiers and
  ordering (ADR 0008 parity).
- Review requests are idempotent and advance the pull-request version only when
  the requested-reviewer set changes. Review submission appends an event and
  does not mutate the pull-request artifact record.
- Workflows that need provider-specific policies must enforce them outside the
  portable `Forge` trait or add a later portable abstraction if the intersection
  becomes clear.

## Alternatives considered

- **Keep review labels.** Rejected: they drift from native review state and
  duplicate facts the Forge already owns.
- **Expose provider mergeability / branch-protection state.** Rejected: it would
  bake provider policy into the portable trait and obscure the workflow's own
  gates.
- **Make reviewer requests executor-driven.** Rejected: the workflow would no
  longer declare who should review, and tests could not reason about the effect
  deterministically.
