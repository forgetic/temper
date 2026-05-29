# Domain model

Harness models the collaboration surface that agentic workflows need in order to plan, execute, review, and merge work.

## Design goals

The model is intentionally small and portable:

- It should represent common Forge concepts without binding workflows to one provider.
- It should preserve stable identity for synchronization and auditing.
- It should keep human-facing numbers and provider-specific opaque identifiers separate.
- It should be rich enough to drive autonomous agents through issues, pull requests, comments, labels, merges, and CI.

## Repositories

A repository is the root collaboration scope. Issues, pull requests, labels, and CI jobs are repository-scoped. A repository has an owner, name, default branch, optional description, and timestamps.

Repositories have a stable `RepositoryId` for synchronization and a human-facing owner/name `RepositoryPath` for lookup and display.

## Users

A user represents an account known to a backend. Users author issues, pull requests, comments, merges, and CI-related actions when a provider exposes that information.

## Issues

An issue represents a unit of tracked work. For agentic workflows, an issue is often the durable task record that describes intent, acceptance criteria, and discussion.

Issues have both:

- a stable `IssueId`, used by machines; and
- an `ItemNumber`, used by humans inside a repository.

Portable workflows can look issues up by repository plus `ItemNumber`, but should store stable IDs for durable synchronization.

## Pull requests

A pull request proposes changes from a source branch to a target branch. Pull requests can be open, closed without merge, or merged.

Pull requests also have both a stable `PullRequestId` and a human-facing `ItemNumber`.
Portable workflows can look pull requests up by repository plus `ItemNumber`, but should store stable IDs for durable synchronization.

Only the merge operation can produce the `merged` state.
Regular pull-request updates may open or close a pull request but cannot mark it merged without a merge record.

## Comments

Comments are append-only discussion messages attached to issues or pull requests. The initial model does not distinguish regular comments from reviews or inline code comments. Those can be added later if the workflow needs them.

## Labels

Labels are repository-scoped metadata used for filtering and routing. Issues and pull requests store label names for simple cross-provider filtering. Label records preserve display metadata such as color and description.

Label assignments behave like sets. Workflows can replace the full label set or apply idempotent add/remove changes when only part of the set should change.

## Merges

A merge records the completion of a pull request. The model stores the merge method, merge commit SHA, merging user, and timestamp. This is enough for workflow auditing without forcing every backend to expose identical low-level Git behavior.

## CI jobs

A CI job represents a provider-reported check for a commit. Jobs are linked to a repository, commit SHA, and optionally a pull request. The status/conclusion split mirrors common CI systems:

- status: queued, running, completed
- conclusion: success, failure, cancelled, skipped, timed out, neutral

## Why a filesystem backend exists

The filesystem backend is the reference backend for development and tests. It lets agents iterate quickly without network access, provider credentials, or rate limits. It should behave like a small deterministic Forge, not like an exact clone of any one provider.

Its local store uses versioned JSON records and a logical clock so repository, issue, pull-request, and comment IDs, timestamps, and list ordering are reproducible in tests.
