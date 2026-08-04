# Forge interface model and query semantics

This page defines the provider-neutral model rules shared by all
`temper_forge::Forge` implementations. Start from [Forge interface
reference](forge-interface.md) for the overview and from
[core artifact operations](forge-interface-artifacts.md) and
[pull-request operations](forge-interface-pull-requests.md) for method-level
behavior.

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

Identifiers are backend-provided opaque strings wrapped in Rust newtypes.
Workflow code must not parse them unless a backend-specific adapter explicitly
owns that logic.

Repositories also have a human-facing `RepositoryPath` made of owner and name.
Issues and pull requests have `ItemNumber`, a human-facing repository-scoped
number. Use these values for display and lookup convenience, not as stable
identity. Do not assume issue and pull-request numbers are cross-type unique on
every backend.

`Forge::item_number_namespace()` makes that distinction explicit without I/O.
Its conservative default is `ItemNumberNamespace::Independent`, where an issue
and a pull request may have the same number. A backend may return `Shared` only
when one repository number identifies at most one artifact across both types.
Wrappers must forward the wrapped backend's value. This capability lets a fresh,
same-pass typed PR candidate resolve an otherwise untyped dependency number
without an issue collision probe; independent backends retain issue-first exact
resolution. Unlisted and cross-repository dependency targets still use summary
exact lookups.

## Lookup semantics

Read lookups return `ForgeResult<Option<T>>`:

- `Ok(Some(value))` means the resource exists and is visible.
- `Ok(None)` means the resource does not exist or is not visible.
- `Err(error)` means the backend could not complete the lookup.

Mutations return `ForgeResult<T>` and should use `ForgeError::NotFound` when the
target resource is missing. Callers that already know an issue or pull-request
number, such as journal recovery, should prefer exact `get_*_by_number` lookups
over broad list queries; an `Ok(None)` target is treated as absent.

## List and query semantics

Ordinary list methods return all visible resources matching the query. Candidate
discovery is the exception: it has an explicit resumable page contract below.
List queries may include one optional sort. Backends must apply the requested
primary sort and deterministic tie-breaks such as item number or stable ID. If
sort is absent, backends should still return deterministic results.

`RepositoryQuery` supports sorting by path, creation time, or update time.
`IssueQuery` and `PullRequestQuery` support state, **conjunctive (all-of)**
labels, exact body substring, author, assignee, detail level, and sorting by
number, creation time, or update time. Supplying `["ready", "urgent"]` to an
ordinary query returns only artifacts carrying both labels; this legacy
contract is not an any-label search.

Consolidated discovery uses the separate `IssueCandidateQuery` and
`PullRequestCandidateQuery` contracts. A candidate query selects one lifecycle
bucket (`Open` or `Terminal`) and either `Unfiltered` or a non-empty
`AnyOf(Vec<String>)` label selection. `Terminal` means closed issues and both
closed and merged pull requests. Candidate detail defaults to `summary()`.
`AnyOf` labels are normalized and deduplicated; results are unioned by typed
stable identity.

Candidate methods return `CandidatePage`, including `items`, `raw_count`,
`returned_count`, `overflow`, `exhausted`, and an optional typed continuation.
A `CandidatePageRequest` limit must be between 1 and 1,000. Rows are ordered by
`updated_at`, item number, and typed stable ID, so equal timestamps have stable
tie-breaks. The first page freezes a high-water boundary in its continuation;
later pages return positions after the prior page but never beyond that
boundary. Concurrent newer additions are deferred to the next sweep instead of
displacing or hiding older eligible rows. Continuations are bound to the
repository, lifecycle, and normalized labels and cannot be reused across
repositories or query shapes. `raw_count` is the number of backend rows
considered before identity deduplication and page truncation; `returned_count`
is exactly the number of returned items.

An open query without `page` remains exhaustive, so every actionable poll stays
level-triggered rather than becoming a newest-only truncation. A terminal query
without an explicit request still receives the fixed default ceiling of 100;
periodic terminal planners attach that request explicitly so the bound is
visible in query plans. Compatibility backends may perform one ordinary
conjunctive list per normalized label/state and apply the observable page
contract after their union.

Unfiltered candidate discovery is intended for open default-kind intake.
Workflow validation rejects a `terminal: true` queue when neither its positive
labels nor its selected artifact kinds' identifying labels can bound discovery.
Periodic planning derives terminal interest only from explicit positive labels
of terminal queues, using identifying labels only for a condition-only terminal
queue. State labels, exclusions, transition effects, and gate labels do not
implicitly become terminal interest. Incomplete journals, durable assignments
and leases, provider/CI recovery, incomplete fan-out, and dependency-gated
recovery remain platform-owned durable evidence rather than generic historical
queue-label drift.

For a backend with provider-side any-label support, one-page request budgets are
therefore constant: broad role discovery and bounded reconciliation each use at
most four populated buckets (issue/PR x open/terminal), independent of workflow
label and configured-role counts. Automated discovery is open-only and adds at
most its populated issue and pull-request buckets. Pagination multiplies each
populated bucket by its page count; it never changes the bucket count or falls
back to per-label lists. Compatibility backends may spend more provider calls
inside a bucket as documented above, but portable callers still issue one
candidate operation per logical bucket.

`CiJobQuery` supports pull request, commit SHA, status, and
sorting by name, creation time, or update time. All populated `CiJobQuery`
filters compose conjunctively: when pull request and commit SHA are both set,
every returned job must belong to that pull request and identify that commit.

Completed jobs retain a typed `CiJobConclusion`. `failure` means an ordinary
job/test failure; cancellation, interruption, timeout, runner loss, startup
failure, action-required, neutral, skipped, and unknown terminalization remain
distinct categories. `unknown` is terminal: adapters must not turn an explicit
but unrecognized terminal provider result back into `queued` or guess that it
was an ordinary failure.

`provider_conclusion` and `provider_reason` preserve printable provider evidence
for diagnostics, bounded to 256 UTF-8 bytes with control characters sanitized.
`run_id` and `attempt` are optional, opaque, repository-scoped provider
identities used to distinguish the latest execution attempt. These additive
fields default to absent so existing serialized filesystem records remain
readable.

`list_ci_jobs_with_presence` returns the same filtered jobs plus a separate
`matching_ci_present` fact. The fact applies the repository, pull-request, and
commit ownership scope, but not the job-status filter. It can therefore be true
while the job list is empty: hosted providers may register a workflow run before
runner capacity materializes any jobs. Callers detecting missing CI must use this
fact rather than infer run absence from an empty job list.

### Body substring filter

Issue and pull-request queries carry `body_contains`, an optional exact
body-substring filter intended for small correlation markers. `Some(non_empty)`
requires the artifact body to contain that exact substring. Matching is
case-sensitive, has no wildcard or regex semantics, and does not search titles
or comments. `None` and `Some("")` are equivalent.

The filter composes with state, labels, author, assignee, sort, and detail
flags. Backends may apply it provider-side or client-side after narrower
provider filters, but observable results, sorting, and detail behavior must
match. A backend must not widen an otherwise narrower provider query just to
apply this portable filter; correlation lookups must still confirm the parsed
metadata key because the filter is only a narrowing hint.

### Issue and pull-request detail levels

Issue and pull-request list queries, candidate queries, plus the
`get_issue_with_details` / `get_issue_by_number_with_details` and
`get_pull_request_with_details` / `get_pull_request_by_number_with_details`
exact variants, carry `ItemListDetails`. The
default is full detail (`dependencies=true`), preserving the historical
contract that results populate native dependency links and provider detail
fields.

Callers that only need scan-summary fields may set `details.dependencies=false`.
Backends may then skip dependency-link enrichment and must return an empty
`dependencies` vector in each listed item. Summary callers should rely only on
artifact identity, number, title/body, state, author, labels, assignees,
timestamps, version, and the empty dependency vector. Exact issue and pull-request summary reads have the same guarantee and are
intended for workflow metadata/checkpoint or target-state recovery that does
not inspect native dependency state.

Pull-request fields that commonly require provider detail rendering — branch
refs, head/base SHAs, requested reviewers, and merge records — may be absent or
empty in summary list results. Use exact `get_*` lookups or full-detail lists
when those fields matter. Runner scans use summary lists with workflow-derived
state/label filters and reload exact artifacts for dependency-gated paths.

## Error categories

Backends map provider-specific failures into `ForgeError`:

- `NotFound`: target resource is missing.
- `AlreadyExists`: create operation conflicts with an existing resource.
- `InvalidRequest`: request is malformed or unsupported by the backend contract.
- `Conflict`: request is valid but cannot be applied because of current state.
- `Backend`: transport, persistence, authentication, authorization, or an
  unexpected provider failure.

Backends with a documented unsupported operation should return `InvalidRequest`
instead of panicking or silently returning incomplete data.
