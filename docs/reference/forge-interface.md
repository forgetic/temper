# Forge interface reference

The Forge interface is the backend-agnostic contract implemented by local files, Forgejo, GitHub, and other collaboration systems.

Rust definition: `crates/temper-forge/src/forge.rs`.

## Contract summary

The `temper_forge::Forge` trait exposes operations for:

- current user identity
- repositories
- repository labels
- issues
- pull requests
- native dependency links between repository items
- comments
- pull-request reviews
- pull-request merges
- CI jobs

All methods are asynchronous because remote providers are expected. Local implementations may use synchronous internals behind the async trait.

## Companion change hints

`ChangeHint`, `ChangeKind`, `ChangeSource`, and `ChangeSourceEvent` are portable
companion types in `temper-forge`, but they are deliberately **not** methods on
the `Forge` trait. A hint source is an optional latency accelerator for runners:
it may be lossy, duplicate, stale, broad, reordered, or closed. Consumers must
use hints only to wake the normal poll path, then re-read Forge state through the
trait before planning or mutating.

Backends and adapters that can emit hints may expose an inherent subscribe/watch
method or wrapper returning a `ChangeSource`. Backends that cannot emit hints are
still complete Forge implementations; periodic polling remains the liveness and
correctness backstop. Review, CI/status, pull-request/head, label, and push hints
are broad enough to wake mechanical landing, but the subsequent scan remains the
authority. Multi-repository runners may narrow an immediate wake tick to
configured repositories named by known hints, but unknown/no-hint batches and
ordinary poll/audit ticks must fall back to the configured scan set.

## Identity model

Every durable resource has a typed stable identifier:

- `RepositoryId`
- `UserId`
- `IssueId`
- `PullRequestId`
- `CommentId`
- `ReviewId`
- `LabelId`
- `CiJobId`

Identifiers are backend-provided opaque strings wrapped in Rust newtypes. Workflow code must not parse them unless a backend-specific adapter explicitly owns that logic.

Repositories also have a human-facing `RepositoryPath` made of owner and name.
Issues and pull requests have `ItemNumber`, a human-facing repository-scoped number.
Use these values for display and lookup convenience, not as stable identity.
Do not assume issue and pull-request numbers are cross-type unique on every backend.

## Lookup semantics

Read lookups return `ForgeResult<Option<T>>`:

- `Ok(Some(value))` means the resource exists and is visible.
- `Ok(None)` means the resource does not exist or is not visible.
- `Err(error)` means the backend could not complete the lookup.

Mutations return `ForgeResult<T>` and should use `ForgeError::NotFound` when the target resource is missing. Callers that already know an issue or pull-request number, such as journal recovery, should prefer exact `get_*_by_number` lookups over broad list queries; an `Ok(None)` target is treated as absent.

## Sorting semantics

List methods return all visible resources matching the query; the current interface has no pagination contract.
List queries may include one optional sort. Backends must apply the requested primary sort and use deterministic tie-breaks such as item number or stable ID.
If sort is absent, backends should still return deterministic results.

Issue and pull-request list queries also carry `body_contains`, an optional
exact body-substring filter intended for small correlation markers. When it is
`Some(non_empty)`, a matching issue or pull request must have a body containing
that exact substring; matching is case-sensitive, has no wildcard or regex
semantics, and does not search titles or comments. `None` and `Some("")` have
identical list-result semantics: no body filter. The filter composes with state,
labels, author, assignee, sort, and detail flags. Backends may apply it
provider-side or client-side after narrower provider filters such as state and
labels, but the observable result set, sorting, and detail behavior must match.
A backend must not widen an otherwise narrower provider query just to apply the
portable body filter; correlation lookups must still confirm the parsed metadata
key because this filter is only a narrowing hint.

Issue and pull-request list queries also carry `ItemListDetails`. The default
is full detail (`dependencies=true`), preserving the historical contract that
list results populate native dependency links and provider detail fields. Callers
that only need scan-summary fields may set `details.dependencies=false`; then
backends may skip dependency link enrichment and must return an empty
`dependencies` vector in each listed item. Summary callers should rely only on
artifact identity, number, title/body, state, author, labels, assignees,
timestamps, version, and the empty dependency vector. Pull-request fields that
commonly require provider detail rendering — branch refs, head/base SHAs,
requested reviewers, and merge records — may be absent or empty in summary list
results; use exact `get_*` lookups or full-detail lists when those fields matter.
Runner and reconciliation candidate scans use summary lists and queue- or
workflow-derived `state`/`labels` filters so normal ticks do not fetch
unlabelled closed history or terminal artifacts carrying only pure
artifact-kind identity labels; dependency-gated paths reload exact artifacts
when they need dependency links. Exact `get_*` lookups and dependency mutation
returns still carry available dependency detail.

## Error categories

Backends map provider-specific failures into `ForgeError`:

- `NotFound`: target resource is missing.
- `AlreadyExists`: create operation conflicts with an existing resource.
- `InvalidRequest`: request is malformed or unsupported by the backend contract.
- `Conflict`: request is valid but cannot be applied because of current state.
- `Backend`: transport, persistence, authentication, authorization, or unexpected provider failure.

Backends with a documented unsupported operation should return `InvalidRequest` instead of panicking or silently returning incomplete data.

## Repository operations

Required methods:

- `list_repositories`
- `create_repository`
- `get_repository`
- `get_repository_by_path`

Repositories expose owner, name, default branch, optional description, and timestamps.
`RepositoryQuery` supports sorting by path, creation time, or update time.

## Label operations

Required methods:

- `list_labels`
- `upsert_label`

Labels are repository-scoped. Issues and pull requests store label names to keep common workflow filters simple and portable.

Label assignment updates are set-like and idempotent. `set_labels` replaces the full label set when present; `add_labels` adds missing labels; `remove_labels` removes present labels. When combined, apply `set_labels`, then removals, then additions.

## Issue operations

Required methods:

- `list_issues`
- `create_issue`
- `get_issue`
- `get_issue_by_number`
- `update_issue`
- `add_issue_dependency`
- `remove_issue_dependency`
- `list_issue_comments`
- `add_issue_comment`

Issue state is `open` or `closed`.

`IssueQuery` supports filtering by state, conjunctive labels, exact body substring, author, assignee, and list detail. Every requested label must be present. Issues can be sorted by number, creation time, or update time.

`Issue::dependencies` lists repository item numbers the issue depends on, sorted deterministically. New issues start with no dependencies. `UpdateIssue` may change title, body, state, labels, and assignees. Closing an open issue sets `closed_at`; reopening a closed issue clears `closed_at`. Label updates apply `set_labels`, then removals, then additions. Assignee changes are idempotent set operations; removals are applied before additions. `UpdateIssue` also carries an optional `expected_version` precondition; see [Optimistic concurrency](#optimistic-concurrency).

## Dependency-link operations

Required methods:

- `add_issue_dependency`
- `remove_issue_dependency`
- `add_pull_request_dependency`
- `remove_pull_request_dependency`

A dependency link means the source issue or pull request is blocked by a target `ItemNumber` in the same repository. Multiple target item numbers are allowed. Links are directed: adding A→B does not add B→A.

Adds require the source to exist and the target item number to resolve to an issue or pull request in the same repository. Missing sources return `ForgeError::NotFound`; add operations also return `NotFound` for missing targets. Removing a missing link is a successful no-op once the source exists, and does not require the target to exist. Dependency lists are set-like, sorted by item number, and contain no duplicates.

A link add/remove that changes the set advances the source artifact's `Version` and `updated_at`. An idempotent no-op returns the current artifact unchanged. Backends whose issue and pull-request numbers are not cross-type unique should treat the target number as existing when either an issue or pull request with that number exists.

Cross-repository dependency aggregation is intentionally not modeled in this trait revision. Workflow-level cross-repo dependencies use repo-qualified metadata projection and resolve each target by reading that target repository freshly. A future trait change may add portable native cross-repo dependency links if enough backends share the same semantics.

## Comment operations

Comments are append-only in the current interface. `add_issue_comment` and `add_pull_request_comment` create comments authored by the backend client's current or provider-authenticated user. Comment list methods must return deterministic results, preferably chronological with stable ID tie-breaks. Supported comment operations return `ForgeError::NotFound` when the target issue or pull request is missing.

## Pull-request operations

Required methods:

- `list_pull_requests`
- `create_pull_request`
- `get_pull_request`
- `get_pull_request_by_number`
- `update_pull_request`
- `add_pull_request_dependency`
- `remove_pull_request_dependency`
- `request_pull_request_reviewers`
- `list_pull_request_reviews`
- `submit_pull_request_review`
- `list_pull_request_comments`
- `add_pull_request_comment`
- `merge_pull_request`

Pull-request state is `open`, `closed`, or `merged`. `PullRequest::dependencies` lists repository item numbers the pull request depends on, sorted deterministically. `PullRequest::requested_reviewers` lists users whose review was requested, sorted deterministically. New pull requests start with no dependencies and no requested reviewers. `UpdatePullRequest` may change title, body, state, labels, and assignees, but may only request `open` or `closed` state changes. Closing an open pull request sets `closed_at`; reopening a closed pull request clears `closed_at`. Label updates apply `set_labels`, then removals, then additions. Assignee changes are idempotent set operations; removals are applied before additions. `UpdatePullRequest` also carries an optional `expected_version` precondition; see [Optimistic concurrency](#optimistic-concurrency).

Merging must go through `merge_pull_request` so the backend can record merge metadata and produce the `merged` state.

`PullRequestQuery` supports filtering by state, conjunctive labels, exact body substring, author, assignee, and list detail. Pull requests can be sorted by number, creation time, or update time.

## Review operations

`request_pull_request_reviewers` adds users to `PullRequest::requested_reviewers` set-like and idempotently. A call that changes the set advances the pull request's `Version` and `updated_at`; a duplicate request is a no-op. The operation returns `ForgeError::NotFound` when the pull request is missing.

`submit_pull_request_review` appends a `PullRequestReview` authored by the backend client's current user. A review has a typed `ReviewId`, `pull_request_id`, `reviewer_id`, `decision`, optional body, and `submitted_at`. Decisions are `approved`, `changes_requested`, `commented`, or `pending`. Submitting a review creates an event and does not mutate the pull-request artifact record or version. `list_pull_request_reviews` returns every verdict event in deterministic chronological order with stable-id tie-breaks, including verdicts a provider later marks dismissed or stale (e.g. a changes-requested review auto-dismissed when the same reviewer approves) — history is preserved, and the latest-per-reviewer aggregate rule below resolves superseding. Only non-verdict events (review requests, pending) are excluded.

The portable aggregate rule is: latest non-comment review per reviewer wins, ordered by `submitted_at` then `ReviewId`. `commented` reviews are ignored for the latest-decision map; `pending` blocks approval without counting as changes requested. A pull request is approved only when at least one reviewer is requested, every requested reviewer's latest non-comment decision is `approved`, and no latest reviewer decision is `changes_requested`. The aggregate reports changes requested when any latest non-comment reviewer decision is `changes_requested`.

Provider-specific review policy is outside this portable contract: CODEOWNERS, required-reviewer rules, stale-review dismissal on push, branch protection, and review threads are not modeled. Backends without those features degrade to requested reviewers plus submitted review events.

## Merge operations

`merge_pull_request` records a `MergeRecord` containing:

- merge method
- merge commit SHA
- merging user
- merge timestamp

Backends that cannot support a requested merge method must return `ForgeError::InvalidRequest` or `ForgeError::Conflict`, depending on whether the method is unsupported in general or unavailable for the current pull request. A merge `Conflict` is a current-state merge rejection only for the merge operation; callers must not treat generic backend conflicts (for example compare-and-swap failures on updates) as merge conflicts. The workflow executor re-reads the pull request after a merge `Conflict`: if it is already merged, at-most-once post-merge projection continues; if it is missing or closed, the target is stale; if it remains open and unmerged, the executor returns a typed workflow-routable merge conflict.

## CI job operations

Required methods:

- `list_ci_jobs`
- `get_ci_job`

CI jobs are associated with a repository, a commit SHA, and optionally a pull request. Status is one of `queued`, `running`, or `completed`. Completed jobs may include a conclusion such as `success`, `failure`, or `cancelled`.

`CiJobQuery` supports filtering by pull request, commit SHA, and status. CI jobs can be sorted by name, creation time, or update time.

## Optimistic concurrency

Issues and pull requests carry a `Version`: a portable, opaque, monotonic
concurrency token (see ADR 0013). A backend assigns `Version::INITIAL` when the
artifact is created and advances it on every successful mutation of the artifact
record — `update_issue`, `update_pull_request`, dependency-link changes,
reviewer-request changes, and `merge_pull_request`. Adding a comment or
submitting a review does not modify the artifact record, so it does not change
the version.

`UpdateIssue` and `UpdatePullRequest` carry an optional `expected_version`
precondition that turns an update into a compare-and-swap:

- `expected_version: None` (the default) — the update is **unconditional**, the
  historical behaviour. The version still advances on success.
- `expected_version: Some(v)` — the update applies **only if** the stored version
  equals `v`. On a match it applies and advances the version; on a mismatch it
  returns `ForgeError::Conflict` and **mutates nothing**. A stale token means the
  artifact changed since the caller read it, so the caller should re-read and
  retry against fresh state.

The token is a dedicated counter, not `updated_at`: reusing a timestamp would
collide whenever two mutations share a clock value and silently admit a lost
update. A real forge can satisfy the precondition with an `ETag`/`If-Match` pair
(the version is the artifact's ETag) or another single atomic claim; the trait
exposes only `Version` and `ForgeError::Conflict`, never provider specifics.

This is the primitive `temper-workflow`'s `LeaseManager` uses to close the
lease-acquisition lost-update window: it captures the version at load and writes
the lease conditionally, so two acquirers over the same "no lease" snapshot
cannot both win. See `docs/reference/robustness-guarantees.md`.

## Idempotency

Create operations (`create_issue`, `create_pull_request`, `add_*_comment`, `submit_pull_request_review`) have no native create-once primitive: each call creates a new resource. Callers that need idempotent creation must implement it above this interface, for example by storing a correlation key in artifact content and searching existing artifacts before creating. The workflow layer does this in `Executor::ensure_issue`, `Executor::ensure_issue_with_parent`, `Executor::ensure_pull_request`, and review idempotency markers; normal create lookups use explicit states, summary detail, available create labels, and a `body_contains` marker before exact metadata confirmation. Reviewer requests are already set-like and idempotent. A future revision may add a portable correlation-key contract if it proves broadly useful.

## Compatibility notes

Concrete backends may expose richer provider-specific features, but portable workflow logic should depend only on this interface. If a provider feature becomes broadly useful, add it to the Forge model with documentation and backend conformance tests.
