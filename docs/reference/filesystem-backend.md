# Filesystem backend reference

The `harness-forge-filesystem` crate implements `harness_forge::Forge` using a local directory tree. It is a deterministic development and test backend, not a production forge. For a faster in-process alternative with the same observable contract, see [the in-memory backend](in-memory-backend.md).

Rust type: `harness_forge_filesystem::FilesystemForge`.

## Supported operations

The current implementation supports:

- `current_user`
- `get_user`
- `list_repositories`
- `create_repository`
- `get_repository`
- `get_repository_by_path`
- `list_labels`
- `upsert_label`
- `list_issues`
- `create_issue`
- `get_issue`
- `get_issue_by_number`
- `update_issue`
- `add_issue_dependency`
- `remove_issue_dependency`
- `list_issue_comments`
- `add_issue_comment`
- `list_pull_requests`
- `create_pull_request`
- `get_pull_request`
- `get_pull_request_by_number`
- `update_pull_request`
- `add_pull_request_dependency`
- `remove_pull_request_dependency`
- `list_pull_request_comments`
- `add_pull_request_comment`
- `merge_pull_request`
- `list_ci_jobs`
- `get_ci_job`

`get_user` only resolves the current user because the Forge interface does not yet include user creation or listing.

## Persistence model

A backend root contains:

```text
metadata.json
repositories/
  repo-0000000000000001.json
  repo-0000000000000001/
    labels.json
    issues.json
    pull_requests.json
    ci_jobs.json
    issues/
      issue-repo-0000000000000001-0000000000000001/
        comments.json
    pull_requests/
      pull-request-repo-0000000000000001-0000000000000001/
        comments.json
  repo-0000000000000002.json
```

`metadata.json` is a versioned JSON record with:

- `schema_version`: currently `1`
- `current_user`: the persisted Forge `User`
- `next_repository_number`: the next numeric repository ID suffix
- `clock_tick`: a deterministic logical clock

Repository files contain serialized Forge `Repository` records. Repository IDs are allocated as `repo-` plus a zero-padded numeric counter. The default bootstrapped user is `user-1` with handle `local`.

Repository timestamps come from the logical clock, not wall-clock time. Each repository creation advances `clock_tick` by one second from the Unix epoch, so tests can rely on deterministic ordering.

Repository owner/name paths are exact and case-sensitive. `create_repository` rejects empty owner, name, or default branch values and returns `ForgeError::AlreadyExists` for duplicate owner/name paths.

Repository labels are stored in `repositories/<repo-id>/labels.json` as serialized Forge `Label` records. Label IDs are deterministic strings derived from the repository ID and hex-encoded label name. Label names are exact and case-sensitive.

Repository issues are stored in `repositories/<repo-id>/issues.json` as serialized Forge `Issue` records. Issue numbers are repository-scoped, start at `1`, and use the next value above the highest stored issue number. Issue IDs are deterministic strings of the form `issue-<repo-id>-<16-digit-number>`. New issues use the current user as `author_id`; labels, assignees, and dependency item numbers are stored as sorted, de-duplicated sets.

Issue timestamps use the same logical clock as repositories. Creating, updating, or changing an issue dependency advances `clock_tick` by one second. Closing an issue sets `closed_at` to the update timestamp; reopening clears `closed_at`. New issues start at `Version::INITIAL`, and every successful `update_issue` or dependency-link change advances the stored `version` (see [Optimistic concurrency](#optimistic-concurrency)). Idempotent dependency no-ops leave the timestamp and version unchanged.

Issue comments are stored in `repositories/<repo-id>/issues/<issue-id>/comments.json` as serialized Forge `Comment` records. Comment IDs are deterministic strings of the form `comment-<issue-id>-<16-digit-number>`. Comment numbers are issue-scoped, start at `1`, and use the next value above the highest stored comment number. New comments use the current user as `author_id`; `created_at` and `updated_at` are the same logical-clock timestamp. Adding a comment advances `clock_tick` by one second and does not modify the stored issue record.

Repository pull requests are stored in `repositories/<repo-id>/pull_requests.json` as serialized Forge `PullRequest` records. Pull-request numbers are repository-scoped within pull requests, start at `1`, and use the next value above the highest stored pull-request number. Pull-request IDs are deterministic strings of the form `pull-request-<repo-id>-<16-digit-number>`. New pull requests use the current user as `author_id`; labels, assignees, and dependency item numbers are stored as sorted, de-duplicated sets. Source and target branch references are stored from the create input. `head_sha`, `base_sha`, and `merge` start as `None`.

Pull-request timestamps use the same logical clock as repositories and issues. Creating, updating, or changing a pull-request dependency advances `clock_tick` by one second. Closing a pull request sets `closed_at` to the update timestamp; reopening clears `closed_at`. New pull requests start at `Version::INITIAL`, and every successful `update_pull_request` or dependency-link change advances the stored `version`. Merging a pull request advances `clock_tick` and the `version`, records a `MergeRecord`, sets state to `merged`, and sets `updated_at` and `closed_at` to the merge timestamp. Merge commit SHAs are deterministic pseudo-SHAs: the logical clock tick formatted as 40 lowercase hexadecimal digits. Idempotent dependency no-ops leave the timestamp and version unchanged.

Pull-request comments are stored in `repositories/<repo-id>/pull_requests/<pull-request-id>/comments.json` as serialized Forge `Comment` records. Comment IDs are deterministic strings of the form `comment-<pull-request-id>-<16-digit-number>`. Comment numbers are pull-request-scoped, start at `1`, and use the next value above the highest stored comment number. New comments use the current user as `author_id`; `created_at` and `updated_at` are the same logical-clock timestamp. Adding a comment advances `clock_tick` by one second and does not modify the stored pull-request record.

CI jobs are stored in `repositories/<repo-id>/ci_jobs.json` as serialized Forge `CiJob` records. The Forge interface has no CI job creation operation, so tests and local scenarios seed this file directly with deterministic fixture records. CI job IDs are fixture-provided opaque IDs. Stored CI jobs must belong to the repository, have non-empty names and commit SHAs, and not duplicate IDs within the repository. CI job timestamps come from the fixture record.

## Listing and sorting

`list_repositories` returns deterministic results. Without an explicit sort, repositories are sorted by owner/name path ascending, then stable ID.

`RepositoryQuery` sorts are supported for:

- `path`
- `created_at`
- `updated_at`

The requested field and direction are applied first. Ties are broken by owner/name path ascending, then stable ID ascending.

`list_labels` returns labels sorted by name ascending, then label ID. `upsert_label` creates or updates one repository-scoped label by exact name and rejects empty label names. Label operations return `ForgeError::NotFound` when the target repository is missing.

`list_issues` supports `IssueQuery` filters for state, conjunctive labels, author ID, and assignee ID. Without a requested sort, issues are sorted by number ascending, then issue ID. `ItemSort` supports number, creation time, and update time with the requested direction; ties use number ascending, then issue ID.

`create_issue` and `list_issues` return `ForgeError::NotFound` when the target repository is missing. `get_issue` returns `Ok(None)` when the issue is not found; `get_issue_by_number` returns `Ok(None)` when the repository or number is not found. `update_issue` and issue dependency updates return `ForgeError::NotFound` when the issue is missing. Adding a dependency also returns `NotFound` when no issue or pull request with the target item number exists in the source repository; removing an absent dependency is a no-op. Issue label updates apply `set_labels`, then removals, then additions. Assignee removals are applied before additions; both are idempotent set operations.

`list_issue_comments` returns comments sorted by creation time ascending, then comment ID. `list_issue_comments` and `add_issue_comment` return `ForgeError::NotFound` when the target issue is missing.

`list_pull_requests` supports `PullRequestQuery` filters for state, conjunctive labels, author ID, and assignee ID. Without a requested sort, pull requests are sorted by number ascending, then pull-request ID. `ItemSort` supports number, creation time, and update time with the requested direction; ties use number ascending, then pull-request ID.

`create_pull_request` and `list_pull_requests` return `ForgeError::NotFound` when the target repository is missing. `get_pull_request` returns `Ok(None)` when the pull request is not found; `get_pull_request_by_number` returns `Ok(None)` when the repository or number is not found. `update_pull_request` and pull-request dependency updates return `ForgeError::NotFound` when the pull request is missing. Adding a dependency also returns `NotFound` when no issue or pull request with the target item number exists in the source repository; removing an absent dependency is a no-op. Pull-request label updates apply `set_labels`, then removals, then additions. Assignee removals are applied before additions; both are idempotent set operations. `UpdatePullRequest` can close and reopen pull requests, but cannot represent merges.

`list_pull_request_comments` returns comments sorted by creation time ascending, then comment ID. `list_pull_request_comments` and `add_pull_request_comment` return `ForgeError::NotFound` when the target pull request is missing.

`merge_pull_request` returns `ForgeError::NotFound` when the target pull request is missing. It supports all current `MergeMethod` values, records the current user as `merged_by`, and stores the requested method. It returns `ForgeError::Conflict` when the pull request is closed or already merged. `commit_title` and `commit_body` are accepted but not persisted because `MergeRecord` has no portable fields for them.

`list_ci_jobs` supports `CiJobQuery` filters for pull-request ID, commit SHA, and status. Without a requested sort, CI jobs are sorted by name ascending, then CI job ID. `CiJobSort` supports name, creation time, and update time with the requested direction; ties use name ascending, then CI job ID. `list_ci_jobs` returns `ForgeError::NotFound` when the target repository is missing. `get_ci_job` returns `Ok(None)` when the job is not found and `ForgeError::Backend` when duplicate CI job IDs exist across stored repositories.

## Unsupported operations

All current Forge trait methods are implemented. The filesystem backend does not expose operations outside the Forge interface, such as creating CI job records; seed `ci_jobs.json` directly for deterministic local scenarios.

## Optimistic concurrency

`update_issue` and `update_pull_request` honour the shared `expected_version` precondition (see ADR 0013 and the [Forge interface reference](forge-interface.md#optimistic-concurrency)). When `expected_version` is `Some` and does not equal the stored `version`, the call returns `ForgeError::Conflict` before advancing the logical clock or writing any file, so a rejected compare-and-swap leaves the store untouched. When it is `None`, the update is unconditional. Stored records carry a `version` field; a record written before versioning existed deserializes as `Version::INITIAL`. The in-memory backend uses the same logic and messages.

## Consistency guarantees

The backend creates its directory layout lazily. It performs single-record writes through a temporary file followed by rename, but it does not use file locks or cross-file transactions. Use it as a single-writer deterministic store for tests and local development.

Malformed JSON, unsupported schema versions, and invalid metadata are reported as `ForgeError::Backend`.
