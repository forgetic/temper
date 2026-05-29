# Filesystem backend reference

The `harness-fs` crate implements `harness_forge::Forge` using a local directory tree. It is the deterministic development and test backend, not a production forge.

Rust type: `harness_fs::FilesystemForge`.

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
- `list_issue_comments`
- `add_issue_comment`
- `list_pull_requests`
- `create_pull_request`
- `get_pull_request`
- `get_pull_request_by_number`
- `update_pull_request`

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
    issues/
      issue-repo-0000000000000001-0000000000000001/
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

Repository issues are stored in `repositories/<repo-id>/issues.json` as serialized Forge `Issue` records. Issue numbers are repository-scoped, start at `1`, and use the next value above the highest stored issue number. Issue IDs are deterministic strings of the form `issue-<repo-id>-<16-digit-number>`. New issues use the current user as `author_id`; labels and assignees are stored as sorted, de-duplicated sets.

Issue timestamps use the same logical clock as repositories. Creating or updating an issue advances `clock_tick` by one second. Closing an issue sets `closed_at` to the update timestamp; reopening clears `closed_at`.

Issue comments are stored in `repositories/<repo-id>/issues/<issue-id>/comments.json` as serialized Forge `Comment` records. Comment IDs are deterministic strings of the form `comment-<issue-id>-<16-digit-number>`. Comment numbers are issue-scoped, start at `1`, and use the next value above the highest stored comment number. New comments use the current user as `author_id`; `created_at` and `updated_at` are the same logical-clock timestamp. Adding a comment advances `clock_tick` by one second and does not modify the stored issue record.

Repository pull requests are stored in `repositories/<repo-id>/pull_requests.json` as serialized Forge `PullRequest` records. Pull-request numbers are repository-scoped within pull requests, start at `1`, and use the next value above the highest stored pull-request number. Pull-request IDs are deterministic strings of the form `pull-request-<repo-id>-<16-digit-number>`. New pull requests use the current user as `author_id`; labels and assignees are stored as sorted, de-duplicated sets. Source and target branch references are stored from the create input. `head_sha`, `base_sha`, and `merge` start as `None`.

Pull-request timestamps use the same logical clock as repositories and issues. Creating or updating a pull request advances `clock_tick` by one second. Closing a pull request sets `closed_at` to the update timestamp; reopening clears `closed_at`. The filesystem backend does not produce the `merged` state until merge support is implemented.

## Listing and sorting

`list_repositories` returns deterministic results. Without an explicit sort, repositories are sorted by owner/name path ascending, then stable ID.

`RepositoryQuery` sorts are supported for:

- `path`
- `created_at`
- `updated_at`

The requested field and direction are applied first. Ties are broken by owner/name path ascending, then stable ID ascending.

`list_labels` returns labels sorted by name ascending, then label ID. `upsert_label` creates or updates one repository-scoped label by exact name and rejects empty label names. Label operations return `ForgeError::NotFound` when the target repository is missing.

`list_issues` supports `IssueQuery` filters for state, conjunctive labels, author ID, and assignee ID. Without a requested sort, issues are sorted by number ascending, then issue ID. `ItemSort` supports number, creation time, and update time with the requested direction; ties use number ascending, then issue ID.

`create_issue` and `list_issues` return `ForgeError::NotFound` when the target repository is missing. `get_issue` returns `Ok(None)` when the issue is not found; `get_issue_by_number` returns `Ok(None)` when the repository or number is not found. `update_issue` returns `ForgeError::NotFound` when the issue is missing. Issue label updates apply `set_labels`, then removals, then additions. Assignee removals are applied before additions; both are idempotent set operations.

`list_issue_comments` returns comments sorted by creation time ascending, then comment ID. `list_issue_comments` and `add_issue_comment` return `ForgeError::NotFound` when the target issue is missing.

`list_pull_requests` supports `PullRequestQuery` filters for state, conjunctive labels, author ID, and assignee ID. Without a requested sort, pull requests are sorted by number ascending, then pull-request ID. `ItemSort` supports number, creation time, and update time with the requested direction; ties use number ascending, then pull-request ID.

`create_pull_request` and `list_pull_requests` return `ForgeError::NotFound` when the target repository is missing. `get_pull_request` returns `Ok(None)` when the pull request is not found; `get_pull_request_by_number` returns `Ok(None)` when the repository or number is not found. `update_pull_request` returns `ForgeError::NotFound` when the pull request is missing. Pull-request label updates apply `set_labels`, then removals, then additions. Assignee removals are applied before additions; both are idempotent set operations. `UpdatePullRequest` can close and reopen pull requests, but merge remains unsupported and cannot be represented by update.

## Unsupported operations

The following Forge areas are not implemented yet:

- pull-request comments
- merges
- CI jobs

Methods in those areas return `ForgeError::InvalidRequest` with a message of the form:

```text
filesystem backend does not support <operation> yet
```

## Consistency guarantees

The backend creates its directory layout lazily. It performs single-record writes through a temporary file followed by rename, but it does not use file locks or cross-file transactions. Use it as a single-writer deterministic store for tests and local development.

Malformed JSON, unsupported schema versions, and invalid metadata are reported as `ForgeError::Backend`.
