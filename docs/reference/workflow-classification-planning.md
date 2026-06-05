# Workflow classification and planning

This page covers the pure read-side workflow layer: metadata parsing, artifact
classification, queue matching/activation, runtime gate signals, and transition
planning. It does not mutate a Forge backend.

## Metadata block format

Workflow data with no portable Forge field lives in a deterministic JSON block
inside an HTML comment in an issue or pull-request body:

```text
<!-- temper:workflow
{
  "kind": "code",
  "parents": [12],
  "dependencies": [{ "repository_id": "forgejo:acme/lib", "number": 34 }],
  "correlation_key": "code-issue-42",
  "lease": {
    "role": "engineer",
    "worker": "run-abc",
    "claimed_at": "2026-05-29T00:00:00Z",
    "heartbeat_at": "2026-05-29T00:05:00Z",
    "expires_at": "2026-05-29T00:30:00Z"
  }
}
-->
```

`render_metadata_block` and `parse_metadata_block` are inverses. Parsing returns
`Ok(None)` when no block exists, `Ok(Some(_))` when it parses, and
`Err(MetadataError)` when a present block is unterminated or invalid. The block
ends at the first `-->`, so values must not contain that sequence.

Relation projections accept bare item numbers as same-repository shorthand or
objects with `repository_id` and `number` for explicit cross-repository targets.
Cross-repo fan-out uses the length-prefixed global child correlation key defined
in [cross-repo workflow contracts](cross-repo-workflows.md).

## Artifact classification

`Classifier` interprets a Forge `Issue` or `PullRequest` under a
`ValidatedWorkflow`. It reads labels, metadata, native dependency links, and
fallback relation metadata; it never mutates Forge state.

Kind resolution uses metadata `kind` when present. Otherwise the classifier
matches `identifying_labels` and chooses the most specific match. State
resolution maps present labels into each state dimension and rejects impossible
exclusive combinations or states illegal for the artifact kind.

Dependency relation resolution prefers native same-repository dependency links.
Metadata dependency fallbacks are used when there are no native same-repo links
and are always preserved for explicit cross-repository targets. Metadata parents
feed `parent` relations; PR parent metadata feeds declared `produced_pr`
relations.

Classification errors are collected, not stopped at the first problem. They
cover unclassified or ambiguous kinds, unknown metadata kinds, target mismatches,
missing identifying labels, exclusive-state conflicts, illegal artifact states,
and malformed metadata.

## Queue evaluation

A classified artifact matches a queue when:

1. its kind is one of the queue's artifact kinds;
2. all common `labels` are present;
3. if `any_of` is present, at least one branch's labels are all present;
4. any queue `condition` is satisfied by classified state or supplied runtime
   signals.

Queue matching is separate from activation. A non-empty queue with no activation
policy is active. With `min_depth` and/or `max_age`, it is active when the member
count reaches `min_depth` or the oldest timestamped member is at least `max_age`
old. `max_age` uses the Forge `updated_at` timestamp; snapshots without a
timestamp cannot satisfy the age branch.

Runner scans derive Forge list queries from subscribed queue interests, prune
cheap label/kind candidates first, and read dependency/CI/review signals only
when a cheap-matched queue or transition needs them. Bounded reconciliation uses
workflow-labelled candidates and exact journal targets; deep audit is the
explicit all-history operator path.

## Gates and runtime signals

A gate may be satisfied by:

- a projected label or state condition;
- labels added by a sibling transition outcome;
- `dependencies_resolved` from fresh dependency target reads;
- native `ci_passed` / `ci_failed` from current-head CI jobs when available;
- native `review_approved` / `review_changes_requested` from requested reviewers
  plus review events.

The planner remains pure. Runtime layers reduce fresh Forge reads into
`GateSignals`, and the planner only tests those signals. Dependency targets that
cannot be read are treated as not landed for that scan. A PR with no readable CI
jobs is not considered passed; in-flight or non-success latest jobs block the
pass gate.

## Transition planning

`plan::Planner` borrows a `ValidatedWorkflow` and computes deterministic plans
without touching a backend. Planning checks, in order:

1. the transition exists;
2. the role is authorized;
3. the artifact kind matches the transition;
4. label-effect preconditions hold (`remove_label` requires presence,
   `add_label` requires absence);
5. required gates are satisfied;
6. applying effects would not create impossible state.

A successful `TransitionPlan` contains the transition, role, artifact kind,
target, ordered effects, and label/assignee postconditions. Comment effects have
no postconditions because comments are append-only; comment idempotency is a
runtime marker check.

`Planner::dependency_unblocks` produces actor-less mechanical plans for blocked
artifacts with declared dependency relations whose dependencies have all landed.
A dependency-gated artifact with no dependency relations is diagnosed rather
than auto-unblocked.
