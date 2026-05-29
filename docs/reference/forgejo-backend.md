# Forgejo backend reference

The `harness-forge-forgejo` crate adapts `harness_forge::Forge` to Forgejo's
HTTP API. It is a **best-effort, offline-tested** backend: the provider is
reached through a mockable HTTP seam, every contract test runs without a
network, and provider semantics that cannot be verified live (notably
conditional writes and merge payload shapes) are documented as best-effort.

Rust type: `harness_forge_forgejo::ForgejoForge<C>`, where `C` is the HTTP
client. Production uses `ReqwestHttpClient`; tests inject a recording mock.

## Configuration and transport

`ForgejoConfig` carries the base URL, the access token, an optional default
repository, the list page size, and the conditional-write mode. Requests are
built by prefixing the path with `/api/v1` and always sending
`Authorization: token <token>`, `Accept: application/json`, and
`Content-Type: application/json`, mirroring the reference TypeScript
integration. List endpoints paginate with `limit`/`page` and stop on the first
short page (bounded by an internal page cap).

## Identifier scheme

Identifiers are backend-private encodings; workflow code must treat them as
opaque and never parse them.

- repository: `forgejo:{owner}/{repo}`
- issue: `forgejo:{owner}/{repo}:issue:{number}`
- pull request: `forgejo:{owner}/{repo}:pull:{number}`
- comment: `forgejo:{owner}/{repo}:comment:{id}`
- review: `forgejo:{owner}/{repo}:review:{id}`
- label: `forgejo:{owner}/{repo}:label:{id}`
- CI job: `forgejo:{owner}/{repo}:actions:{run}:{job_index}:{task_id}`
- user: the Forgejo login, unprefixed (so reviewer-request logins map directly)

## Implemented operations

This phase implements the pull-request surface. Repository, label, issue,
dependency, and CI operations are added in their own phases.

- `list_pull_requests`, `get_pull_request`, `get_pull_request_by_number`
- `create_pull_request`, `update_pull_request`
- `list_pull_request_comments`, `add_pull_request_comment`
- `request_pull_request_reviewers`
- `list_pull_request_reviews`, `submit_pull_request_review`
- `merge_pull_request`

## Pull requests

`list_pull_requests` calls `GET /repos/{owner}/{repo}/pulls?state=...`. The
portable state filter maps `Open → open` and both `Closed` and `Merged →
closed`; `None → all`. Forgejo's `/pulls` endpoint has no label filter, so
label, author, assignee, and the exact portable state filter are applied
client-side after mapping. Results sort by the requested sort field, then by
number, then by id for determinism.

`get_pull_request`/`get_pull_request_by_number` call `GET /pulls/{number}`; a
`404` maps to `Ok(None)`.

`create_pull_request` posts `{ title, head, base, body }` to `POST /pulls`,
then applies labels and assignees through the issue endpoints (see below) and
re-fetches the pull request so the returned value reflects the applied
metadata. Empty label/assignee sets skip the corresponding requests.

`update_pull_request` re-reads the current pull request, performs the optional
conditional-write check (below), patches `title`/`body`/`state` through
`PATCH /pulls/{number}` when any is set, applies label/assignee changes, and
re-fetches. A missing pull request maps to `NotFound`.

### Label and assignee sequencing (shared with issues)

Pull requests are issues on Forgejo, so label and assignee updates use the
issue endpoints and the same helper issues use, keeping one sequencing
implementation:

- labels: `set_labels` replaces the full set with `PUT
  /issues/{number}/labels`; removals are deleted by their numeric label id with
  `DELETE /issues/{number}/labels/{id}` (a missing label is a no-op); additions
  are appended with `POST /issues/{number}/labels`.
- assignees: the new set is computed as `current − remove + add` (sorted,
  deduplicated) and written with `PATCH /issues/{number}`; a no-op update skips
  the request.

## Pull-request comments

Forgejo PR comments are issue comments. `list_pull_request_comments` and
`add_pull_request_comment` use `GET`/`POST /issues/{number}/comments` and map
exactly like issue comments. A missing item maps to `NotFound`.

## Requested reviewers

`request_pull_request_reviewers` posts `{ reviewers: [login...],
team_reviewers: [] }` to `POST /pulls/{number}/requested_reviewers`, then
re-fetches the pull request. User ids are logins, so they map directly. The
call is idempotent: Forgejo rejects re-requesting an already-requested
reviewer, so on a non-success response the backend re-fetches and returns the
current pull request when the desired reviewers are already present. A `404`
maps to `NotFound`.

## Reviews

`list_pull_request_reviews` calls `GET /pulls/{number}/reviews` and maps
provider states to portable decisions, accepting both submit-event and stored
state spellings: `APPROVED`/`approve → Approved`,
`REQUEST_CHANGES`/`changes_requested → ChangesRequested`,
`COMMENT`/`commented → Commented`, `PENDING → Pending`. Dismissed and stale
reviews, review-request events (`REQUEST_REVIEW`), and unknown states are
excluded from the portable list so they cannot affect the portable review
aggregate (`PullRequestReviewStatus`). Reviews sort by submission time, then id.

`submit_pull_request_review` submits in **one call**:
`POST /pulls/{number}/reviews` with `{ event, body }` where `event` is
`APPROVED`, `REQUEST_CHANGES`, or `COMMENT`. The author is the backend client's
current user. `ReviewDecision::Pending` returns `InvalidRequest`: the historical
two-step pending flow drops the body for `APPROVED`, so it is deliberately not
used. If the provider's response echo is too sparse to map, the backend returns
a review carrying the decision it submitted.

## Merge

`merge_pull_request` posts to `POST /pulls/{number}/merge` with a best-effort
Gitea/Forgejo payload: `{ Do, MergeTitleField?, MergeMessageField? }`, where
`Do` maps `MergeCommit → merge`, `Squash → squash`, `Rebase → rebase`. These
field names are not verified against a live instance and may need refinement.
The merge `POST` returns no usable body, so the backend re-fetches the pull
request for the merge commit SHA, merger, and timestamp; the returned
`MergeRecord` reports the method that was requested. A success with no merge
record maps to `Backend`. `404` maps to `NotFound`; `405`/`409`/`412`/`422`
(already merged, not mergeable, failed precondition) map to `Conflict`.

## Optimistic concurrency (best-effort)

Forgejo exposes no confirmed conditional-write contract, so compare-and-swap is
best-effort. The backend derives a portable `Version` from a per-artifact
validator — an `ETag` header when present, otherwise the weak `updated_at`
timestamp — captured on every read through a shared `VersionCache`. A
`Version` is only meaningful when the read that issued it and the conditional
write that consumes it go through the same backend instance, which is how the
workflow layer's `LeaseManager` uses it.

When `update_pull_request` is called with `expected_version`, the backend
re-reads the artifact and re-resolves its validator before mutating; a changed
validator returns `Conflict` and writes nothing. When no validator is
available, `CasMode::Strict` refuses the conditional write
(`InvalidRequest`) while `CasMode::BestEffort` proceeds with a documented weak
read-before-write.

Residual races: read-modify-write is not atomic, and `updated_at` has
one-second granularity. Until live validation confirms provider-supported
conditional requests, lease-race safety on this backend is best-effort and
mode-dependent.
