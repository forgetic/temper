# Forge interface core artifact operations

This page defines method-level behavior for the portable `Forge` trait. See
[model and query semantics](forge-interface-model.md) for identifiers, list
filters, detail levels, sorting, and error categories.

## User operations

Required methods:

- `current_user`
- `get_user`

`current_user` returns the identity used by this backend client. `get_user`
looks up a user by stable backend ID; backends may only know users visible to the
client.

## Repository operations

Required methods:

- `list_repositories`
- `create_repository`
- `get_repository`
- `get_repository_by_path`

Repositories expose owner, name, default branch, optional description, and
timestamps.

## Label operations

Required methods:

- `list_labels`
- `upsert_label`

Labels are repository-scoped. Issues and pull requests store label names to keep
workflow filters simple and portable.

Label assignment updates are set-like and idempotent. `set_labels` replaces the
full label set when present; `add_labels` adds missing labels; `remove_labels`
removes present labels. When combined, apply `set_labels`, then removals, then
additions.

## Issue operations

Required methods:

- `list_issues`
- `list_issue_candidates`
- `create_issue`
- `get_issue`
- `get_issue_with_details`
- `get_issue_by_number`
- `get_issue_by_number_with_details`
- `update_issue`
- `update_issue_from_snapshot`
- `add_issue_dependency`
- `remove_issue_dependency`
- `list_issue_comments`
- `add_issue_comment`

Issue state is `open` or `closed`. New issues start with no dependencies.
`Issue::dependencies` lists repository item numbers the issue depends on, sorted
deterministically.

`UpdateIssue` may change title, body, state, labels, and assignees. Closing an
open issue sets `closed_at`; reopening a closed issue clears `closed_at`. Label
updates apply `set_labels`, then removals, then additions. Assignee removals are
applied before additions. `UpdateIssue` also carries `expected_version`; see
[optimistic concurrency](forge-interface-concurrency.md).

The `*_with_details` exact reads for both issues and pull requests accept the
same `ItemListDetails` budget as list and candidate queries. `summary()` returns
the complete workflow/body representation but may omit native dependency
enrichment. The historical exact methods retain full detail. Candidate reads
are lifecycle-bucketed any-label discovery; ordinary list labels remain
conjunctive. Open queries are level-triggered. Terminal queries return frozen,
typed continuation pages and expose raw/returned/overflow/completion counts.
Production hosted adapters must impose a documented fixed provider boundary per
terminal operation. Forgejo decodes at most 1,001 rows and performs at most 64
list requests; a default 100-row PR page may add 100 exact summary reads for
ambiguous merge markers, giving a 164-request worst case. Overflow resumes in a
later generation rather than extending one periodic pass. The full ordering, restart, invalidation, memory, and deep-audit contract is in
[the interface model](forge-interface-model.md).

`update_issue_from_snapshot(current, input)` carries a previously validated
`Issue` into a mutation. Successful calls return the committed representation
that callers should pass to the next phase. Backends may use `current` for label
and assignee replacement and the provider mutation response for body/version,
avoiding unconditional read-before-write and write-after-read amplification.
The compatibility default delegates to `update_issue`; native hosted backends
retain only a preflight read required for a conditional update.

## Dependency-link operations

Required methods:

- `add_issue_dependency`
- `remove_issue_dependency`
- `add_pull_request_dependency`
- `remove_pull_request_dependency`

A dependency link means the source issue or pull request is blocked by a target
`ItemNumber` in the same repository. Links are directed: adding A→B does not add
B→A. Multiple targets are allowed; dependency lists are set-like, sorted by item
number, and contain no duplicates. Because the link itself is untyped, target
state resolution is issue-first on `ItemNumberNamespace::Independent` backends.
A fresh typed candidate may bypass that probe only when the backend advertises a
`Shared` issue/PR namespace.

Adds require the source to exist and the target item number to resolve to an
issue or pull request in the same repository. Missing sources return
`ForgeError::NotFound`; add operations also return `NotFound` for missing
targets. Removing a missing link is a successful no-op once the source exists
and does not require the target to exist.

A link add/remove that changes the set advances the source artifact's `Version`
and `updated_at`. An idempotent no-op returns the current artifact unchanged.
Backends whose issue and pull-request numbers are not cross-type unique should
treat the target number as existing when either artifact type has that number.

Cross-repository dependency aggregation is intentionally not modeled in this
trait revision. Workflow-level cross-repo dependencies use repo-qualified
metadata projection and resolve each target repository freshly. A future trait
change may add portable native cross-repo dependency links if enough backends
share the same semantics.

## Comment operations

Comments are append-only in the current interface. `add_issue_comment` and
`add_pull_request_comment` create comments authored by the backend client's
current or provider-authenticated user. Comment list methods must return
deterministic results, preferably chronological with stable ID tie-breaks.
Supported comment operations return `ForgeError::NotFound` when the target issue
or pull request is missing.

These methods model ordinary issue and pull-request conversation comments only.
They do not expose provider label changes, activity feeds, or hidden timeline
records. Forgejo's internal representation may use comment-like timeline rows
for label events, but those rows are not portable `Comment` values. The durable
plan-validation audit is deliberately an ordinary coordinating-plan comment, so
operators and agents can retrieve it through this contract without consulting
Temper journals, Forgejo SQLite, or provider timeline internals.

## Pull requests, reviews, merges, and CI

Pull-request-adjacent methods have their own focused page:
[pull-request, review, merge, and CI operations](forge-interface-pull-requests.md).
Pull-request dependency and comment operations follow the same portable rules as
issue dependency links and comments.
