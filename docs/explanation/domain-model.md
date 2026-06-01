# Domain model

Harness models the collaboration surface that agentic workflows need in order to plan, execute, review, and merge work.

## Design goals

The model is intentionally small and portable:

- It should represent common Forge concepts without binding workflows to one provider.
- It should preserve stable identity for synchronization and auditing.
- It should keep human-facing numbers and provider-specific opaque identifiers separate.
- It should be rich enough to drive autonomous agents through issues, pull requests, comments, reviews, labels, merges, and CI.

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

## Artifact references

The workflow layer names linked issues and pull requests with an `ArtifactRef`: a portable repository-qualified reference made from a `RepositoryId` and an `ItemNumber`. Existing same-repository links may omit the repository id as a shorthand; readers resolve that default against the artifact carrying the link. Cross-repository links use the explicit repository id.

Only the merge operation can produce the `merged` state.
Regular pull-request updates may open or close a pull request but cannot mark it merged without a merge record.

## Concurrency tokens

Issues and pull requests carry a `Version`: an opaque, monotonic optimistic-concurrency token. It advances on every successful mutation of the artifact record, so a caller can capture the version at read time and pass it back as a conditional-update precondition. The update applies only if the stored version still matches; otherwise it is a conflict and nothing changes. This is a portable optimistic-concurrency primitive (an ETag-style row version), not a timestamp, so it never collides when two mutations share a clock value. See ADR 0013 and the [Forge interface reference](../reference/forge-interface.md).

## Workflow relations

Relations are workflow-level links between artifact kinds. A workflow declares allowed relation kinds such as `parent`, `dependency`, and `produced_pr` between artifact kinds.

Dependency links are now native Forge state for same-repository dependencies: issues and pull requests carry the repository item numbers they depend on, and Forge operations add or remove those links idempotently. The workflow classifier combines native dependency numbers with relation declarations to produce typed same-repository `dependency` relations.

`parent` and `produced_pr` remain metadata-projected because they do not share a portable provider-native representation. Metadata relation fields can carry same-repository item numbers or explicit `ArtifactRef` objects; the metadata `dependencies` field remains a fallback for older artifacts that have no native dependency links.

## Pull-request reviews

Reviews are native pull-request state, separate from ordinary PR comments. A pull request records requested reviewers and append-only review events. Each event has a reviewer, a portable decision (`approved`, `changes_requested`, `commented`, or `pending`), optional body, and submission time.

The portable aggregate is intentionally small: the latest non-comment decision per reviewer wins; a PR is approved when every requested reviewer's latest decision approves and none request changes. Provider-specific policy such as CODEOWNERS, stale-review dismissal, branch protection, and review threads stays outside the portable model.

## Comments

Comments are append-only discussion messages attached to issues or pull requests. They are separate from native reviews and inline code-review threads.

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

Its local store uses versioned JSON records, a logical clock, and deterministic fixtures so repository, issue, pull-request, comment, merge, and CI job metadata stay reproducible in tests.
