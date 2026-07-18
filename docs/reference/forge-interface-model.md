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

List methods return all visible resources matching the query; the interface has
no pagination contract. List queries may include one optional sort. Backends
must apply the requested primary sort and deterministic tie-breaks such as item
number or stable ID. If sort is absent, backends should still return
deterministic results.

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
stable identity and sorted by item number then stable ID. Compatibility
backends may perform one ordinary conjunctive list per normalized label/state.
Thus candidate reads can intentionally return a superset while queue matching
continues to enforce label conjunctions and any-of branches locally.

Unfiltered candidate discovery is intended for open default-kind intake.
Bounded workflow planning never emits an unfiltered terminal bucket, avoiding
unlabelled closed/merged history reads.

`CiJobQuery` supports pull request, commit SHA, status, and
sorting by name, creation time, or update time. All populated `CiJobQuery`
filters compose conjunctively: when pull request and commit SHA are both set,
every returned job must belong to that pull request and identify that commit.

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
