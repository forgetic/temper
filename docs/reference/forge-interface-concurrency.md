# Forge interface optimistic concurrency and idempotency

This page defines the portable conditional-write and idempotency rules for the
`Forge` trait. See [ADR 0013](../adr/0013-portable-optimistic-concurrency.md)
for the decision record. Use [core artifact operations](forge-interface-artifacts.md)
and [pull-request operations](forge-interface-pull-requests.md) for method-level
behavior.

## Optimistic concurrency

Issues and pull requests carry a `Version`: a portable, opaque, monotonic
concurrency token. A backend assigns `Version::INITIAL` when the artifact is
created and advances it on every successful mutation of the artifact record:

- `update_issue` and `update_pull_request`
- dependency-link changes
- reviewer-request changes
- `merge_pull_request`

Adding a comment or submitting a review does not modify the artifact record, so
it does not change the version.

`UpdateIssue` and `UpdatePullRequest` carry an optional `expected_version`
precondition that turns an update into a compare-and-swap:

- `expected_version: None` — the update is unconditional, preserving historical
  behavior. The version still advances on success.
- `expected_version: Some(v)` — the update applies only if the stored version
  equals `v`. On a match it applies and advances the version; on a mismatch it
  returns `ForgeError::Conflict` and mutates nothing.

A stale token means the artifact changed since the caller read it, so the caller
should re-read and retry against fresh state.

## Why the token is not `updated_at`

The token is a dedicated counter, not a timestamp. Reusing `updated_at` would
collide whenever two mutations share a clock value and could silently admit a
lost update. A real forge can satisfy the precondition with an `ETag`/`If-Match`
pair or another single atomic claim; the trait exposes only `Version` and
`ForgeError::Conflict`, never provider specifics.

This is the primitive `temper-workflow`'s `LeaseManager` uses to close the
lease-acquisition lost-update window: it captures the version at load and writes
the lease conditionally, so two acquirers over the same "no lease" snapshot
cannot both win. See [robustness guarantees](robustness-guarantees.md) for the
workflow-level invariant.

## Idempotency

Create operations have no native create-once primitive:

- `create_issue`
- `create_pull_request`
- `add_*_comment`
- `submit_pull_request_review`

Each call creates a new resource. Callers that need idempotent creation must
implement it above the interface, for example by storing a correlation key in
artifact content and searching existing artifacts before creating. The workflow
layer does this for issue, pull-request, and review creation paths by using
explicit states, summary detail, create labels, `body_contains` markers, and
exact metadata confirmation.

Reviewer requests and dependency links are already set-like and idempotent. A
future revision may add a portable correlation-key contract if it proves broadly
useful.

## Compatibility notes

The change is backward compatible: `expected_version` defaults to `None`, the
`version` field is `#[serde(default)]`, and a pre-versioning record reads as
`Version::INITIAL`.
