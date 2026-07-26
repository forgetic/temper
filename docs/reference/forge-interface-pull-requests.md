# Forge interface pull-request, review, merge, and CI operations

This page defines pull-request-adjacent behavior for the portable `Forge` trait.
See [model and query semantics](forge-interface-model.md) for identifiers, list
filters, detail levels, sorting, and error categories, and
[core artifact operations](forge-interface-artifacts.md) for repositories,
issues, dependencies, and comments.

## Pull-request operations

Required methods:

- `list_pull_requests`
- `list_pull_request_candidates`
- `create_pull_request`
- `get_pull_request`
- `get_pull_request_with_details`
- `get_pull_request_by_number`
- `get_pull_request_by_number_with_details`
- `update_pull_request`
- `add_pull_request_dependency`
- `remove_pull_request_dependency`
- `request_pull_request_reviewers`
- `list_pull_request_reviews`
- `submit_pull_request_review`
- `list_pull_request_comments`
- `add_pull_request_comment`
- `merge_pull_request`

Pull-request state is `open`, `closed`, or `merged`. New pull requests start
with no dependencies and no requested reviewers. `PullRequest::dependencies` and
`PullRequest::requested_reviewers` are sorted deterministic sets.

`UpdatePullRequest` may change title, body, labels, assignees, and only `open` or
`closed` state. Closing an open pull request sets `closed_at`; reopening a closed
pull request clears it. Merging must go through `merge_pull_request` so the
backend can record merge metadata and produce the `merged` state.
`UpdatePullRequest` also carries `expected_version`; see
[optimistic concurrency](forge-interface-concurrency.md).

Pull-request dependency and comment operations follow the same portable rules as
issue dependency links and comments.

## Review operations

`request_pull_request_reviewers` adds users to `PullRequest::requested_reviewers`
set-like and idempotently. A call that changes the set advances the pull
request's `Version` and `updated_at`; a duplicate request is a no-op. The
operation returns `ForgeError::NotFound` when the pull request is missing.

`submit_pull_request_review` appends a `PullRequestReview` authored by the
backend client's current user. A review has a typed `ReviewId`, `pull_request_id`,
`reviewer_id`, `decision`, optional body, and `submitted_at`. Decisions are
`approved`, `changes_requested`, `commented`, or `pending`. Submitting a review
creates an event and does not mutate the pull-request artifact record or version.

`list_pull_request_reviews` returns every verdict event in deterministic
chronological order with stable-ID tie-breaks, including verdicts a provider
later marks dismissed or stale. Only non-verdict events such as review requests
are excluded.

The portable aggregate rule is: latest non-comment review per reviewer wins,
ordered by `submitted_at` then `ReviewId`. `commented` reviews are ignored for
the latest-decision map; `pending` blocks approval without counting as changes
requested. A pull request is approved only when at least one reviewer is
requested, every requested reviewer's latest non-comment decision is `approved`,
and no latest reviewer decision is `changes_requested`. The aggregate reports
changes requested when any latest non-comment reviewer decision is
`changes_requested`.

Provider-specific review policy is outside this portable contract: CODEOWNERS,
required-reviewer rules, stale-review dismissal on push, branch protection, and
review threads are not modeled. Backends without those features degrade to
requested reviewers plus submitted review events.

## Merge operations

`merge_pull_request` records a `MergeRecord` containing merge method, merge
commit SHA, merging user, and merge timestamp.

Backends that cannot support a requested merge method must return
`ForgeError::InvalidRequest` if unsupported in general or `ForgeError::Conflict`
if unavailable for the current pull request. A merge `Conflict` is specific to
`merge_pull_request`; callers must not treat generic backend conflicts, such as
compare-and-swap failures on updates, as merge conflicts. The workflow executor
re-reads the pull request after a merge `Conflict`: an already merged pull
request continues post-merge projection; missing or closed is stale; still-open
and unmerged becomes a typed workflow-routable merge conflict.

## CI job operations

Required methods:

- `list_ci_jobs`
- `list_ci_jobs_with_presence`
- `get_ci_job`
- `retry_ci_attempt`

CI jobs are associated with a repository, a commit SHA, and optionally a pull
request. Status is `queued`, `running`, or `completed`. Completed jobs carry a
typed category: success, ordinary failure, cancellation, interruption, timeout,
runner loss, startup failure, action-required, neutral, skipped, or unknown
terminalization. Unknown terminalization remains completed and is not evidence
of an ordinary source/test failure. Sanitized provider conclusion/reason strings
and opaque run/attempt identities are retained when the provider exposes them;
all four fields are optional for backward-compatible stored records. Populated
`CiJobQuery` filters are conjunctive, so a query containing both pull-request ID
and commit SHA returns only jobs satisfying both constraints.

`list_ci_jobs_with_presence` also reports whether provider evidence matched CI
for the query ownership scope. That fact remains true when a workflow run is
registered but has no assigned jobs yet, and is independent of a job-status
filter that removes every returned job. `list_ci_jobs` remains the jobs-only
convenience operation for callers that do not reason about missing CI.

## Exact-attempt CI retry

`CiRetryRequest::new` requires a repository, pull request, non-empty exact head
SHA, opaque run and attempt identities, and at least one freshly read job. Every
job must carry those same repository/PR/head/run/attempt coordinates. The
constructor canonicalizes the authoritative latest job set into a deterministic
fingerprint containing stable job identity, status, typed and provider terminal
evidence, and update time. Empty sets, duplicate IDs, and widened coordinates
are rejected.

`retry_ci_attempt` is a fenced provider side effect, not a generic workflow
trigger. Before mutation a backend must re-read the pull request head, exact run
and attempt, and latest job set and compare every coordinate and the fingerprint.
It returns one of five typed outcomes:

- `accepted`: the provider acknowledged this exact retry;
- `already_observed`: a receipt or newer provider attempt proves a retry is
  already visible, so no duplicate mutation was made;
- `unsupported`: the backend/version has no endpoint with verified semantics;
- `rejected(reason)`: a stale identity/fingerprint, non-retryable run, or
  explicit provider rejection prevented mutation;
- `uncertain`: the backend cannot prove whether the single provider request
  took effect, for example after a transport loss or `5xx` response.

`uncertain` is not permission for an unbounded duplicate. The caller must retain
its interruption evidence and perform a later authoritative exact-head read. A
new attempt reconciles the request as already observed; unchanged evidence may
be retried only under the caller's separate bounded recovery policy.

Capability detection fails closed. A backend must report `unsupported` rather
than guessing an undocumented endpoint or using a UI/read fallback for writes.
No implementation may create an empty commit, force-push, update a ref, or make
any other source mutation to trigger CI.

Current provider support is intentionally narrow:

| Backend | Exact-attempt retry | Fallback behavior |
| --- | --- | --- |
| GitHub | Supported through the documented Actions run rerun endpoint after exact head/attempt/job-set revalidation | `404`/`410` is unsupported; transport/`5xx` is uncertain and is not blindly repeated |
| Forgejo | Unsupported on all supported versions | Use a configured read-only diagnostic or park with evidence; never guess a rerun endpoint |
| Memory/filesystem | Deterministic test outcome / unsupported reference behavior | Never mutate commits or refs |

Provider support decides only whether the first bounded action can be attempted.
Unsupported, rejected, uncertain, and exhausted outcomes retain the same typed
terminal evidence for diagnostic fallback and parking.
