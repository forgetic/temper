# Forge interface reference

The Forge interface is the backend-agnostic contract implemented by local files, Forgejo, GitHub, and other collaboration systems.

Rust definition: `crates/harness-forge/src/forge.rs`.

## Contract summary

A backend implementing `harness_forge::Forge` must provide operations for:

- current user identity
- repositories
- repository labels
- issues
- pull requests
- comments
- pull-request merges
- CI jobs

All methods are asynchronous because remote providers are expected. Local implementations may use synchronous internals behind the async trait.

## Identity model

Every durable resource has a typed stable identifier:

- `RepositoryId`
- `UserId`
- `IssueId`
- `PullRequestId`
- `CommentId`
- `LabelId`
- `CiJobId`

Identifiers are backend-provided opaque strings wrapped in Rust newtypes. Workflow code must not parse them unless a backend-specific adapter explicitly owns that logic.

Issues and pull requests also have `ItemNumber`, a human-facing repository-scoped number. These numbers are for display and lookup convenience, not for global identity.

## Lookup semantics

Read lookups return `ForgeResult<Option<T>>`:

- `Ok(Some(value))` means the resource exists and is visible.
- `Ok(None)` means the resource does not exist or is not visible.
- `Err(error)` means the backend could not complete the lookup.

Mutations return `ForgeResult<T>` and should use `ForgeError::NotFound` when the target resource is missing.

## Error categories

Backends map provider-specific failures into `ForgeError`:

- `NotFound`: target resource is missing.
- `AlreadyExists`: create operation conflicts with an existing resource.
- `InvalidRequest`: request is malformed or unsupported by the backend contract.
- `Conflict`: request is valid but cannot be applied because of current state.
- `Backend`: transport, persistence, authentication, authorization, or unexpected provider failure.

## Repository operations

Required methods:

- `list_repositories`
- `create_repository`
- `get_repository`

Repositories expose owner, name, default branch, optional description, and timestamps.

## Label operations

Required methods:

- `list_labels`
- `upsert_label`

Labels are repository-scoped. Issues and pull requests store label names to keep common workflow filters simple and portable.

## Issue operations

Required methods:

- `list_issues`
- `create_issue`
- `get_issue`
- `update_issue`
- `list_issue_comments`
- `add_issue_comment`

Issue state is `open` or `closed`.

`IssueQuery` supports filtering by state, labels, author, and assignee. Label filtering is conjunctive: every requested label must be present.

## Pull-request operations

Required methods:

- `list_pull_requests`
- `create_pull_request`
- `get_pull_request`
- `update_pull_request`
- `list_pull_request_comments`
- `add_pull_request_comment`
- `merge_pull_request`

Pull-request state is `open`, `closed`, or `merged`.

`PullRequestQuery` supports filtering by state, labels, author, and assignee. Label filtering is conjunctive.

## Merge operations

`merge_pull_request` records a `MergeRecord` containing:

- merge method
- merge commit SHA
- merging user
- merge timestamp

Backends that cannot support a requested merge method must return `ForgeError::InvalidRequest` or `ForgeError::Conflict`, depending on whether the method is unsupported in general or unavailable for the current pull request.

## CI job operations

Required methods:

- `list_ci_jobs`
- `get_ci_job`

CI jobs are associated with a repository, a commit SHA, and optionally a pull request. Status is one of `queued`, `running`, or `completed`. Completed jobs may include a conclusion such as `success`, `failure`, or `cancelled`.

## Compatibility notes

Concrete backends may expose richer provider-specific features, but portable workflow logic should depend only on this interface. If a provider feature becomes broadly useful, add it to the Forge model with documentation and backend conformance tests.
