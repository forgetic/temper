# In-memory backend reference

The `temper-forge-memory` crate implements `temper_forge::Forge` entirely in
process. All records live in ordinary collections behind a single mutex, with no
filesystem, network, or async runtime involved. It is a reference development and
test backend, not a production forge.

Rust type: `temper_forge_memory::MemoryForge`.

It is a sibling to the filesystem backend (see ADR 0008) and intentionally
reproduces the same observable contract so tests can swap between them. The
contract below records only what is specific to the in-memory backend; the
shared semantics (operations, identifiers, logical clock, ordering, query
filters including `body_contains`, error mapping) are defined in
[the filesystem backend reference](filesystem-backend.md) and apply identically
here unless stated otherwise.

## Supported operations

Every `Forge` trait method is implemented, matching the filesystem backend's
supported set, including native dependency links and pull-request reviews.
`get_user` only resolves the current user, because the Forge interface has no
user creation or listing.

## Persistence model

There is no durable store. State lives in memory for the lifetime of the backend
instance:

- store `current_user`: the bootstrapped Forge `User` (default `user-1` /
  `local`) used by handles without an `as_user` override.
- a logical `clock_tick` starting at `0`.
- a repository-id counter starting at `1`.
- in-memory collections of repositories, labels, issues, pull requests, issue
  comments, pull-request comments, pull-request reviews, CI jobs, and explicit
  queued CI-run fixtures;
  dependency item numbers live on the issue and pull-request records, and
  requested reviewers live on pull-request records.

Identifiers and timestamps match the filesystem backend exactly: repository IDs
are `repo-` plus a zero-padded counter; issue, pull-request, comment, review,
and label IDs use the same deterministic schemes; each mutating operation that
needs a timestamp advances `clock_tick` by one second from the Unix epoch; merge
commit SHAs are the clock tick formatted as 40 lowercase hexadecimal digits.

Version tokens also match the filesystem backend: a created issue or pull request
starts at `Version::INITIAL`, and `update_issue`, `update_pull_request`,
dependency-link changes, reviewer-request changes, and `merge_pull_request` each
advance the artifact's version. Adding a comment, submitting a review, and
idempotent dependency or reviewer-request no-ops do not change the artifact
record, so they leave the version untouched.

`create_repository` validates non-empty owner, name, and default branch, and
returns `ForgeError::AlreadyExists` for duplicate owner/name paths.
`upsert_label` rejects empty label names.

## Cloning shares one store

`MemoryForge` is `Clone`. A clone shares the same underlying store through an
`Arc`, so a clone observes and mutates the same records. This lets a test hand
the backend to several helpers while keeping one logical store.

## In-process change hints

`MemoryForge::subscribe_hints()` is an optional companion surface returning a
`MemoryHintReceiver`, which implements `temper_forge::ChangeSource`. The
`temper_forge::factory::new_memory_with_change_source()` facade constructor
returns the same capability paired with an abstract `Arc<dyn Forge>` for tests
that should not depend on memory-backend internals. Successful mutations publish
best-effort `ChangeHint`s to subscribers on the same shared store, including
fixture CI changes from `seed_ci_jobs`. Failed operations, rejected
optimistic-concurrency preconditions, and idempotent no-op dependency or
reviewer-request calls do not publish because no state changed.

Hints are never authoritative state. Tests and runners use them only to wake the
normal Forge scan; the worker still re-reads issues, pull requests, reviews, CI,
and labels before acting.

## Per-handle identity

`MemoryForge::as_user(user)` returns another handle over the same shared store
but with a handle-local `current_user` override. Clones of that handle preserve
the override, while other handles can report and act as different users. This
matches the filesystem backend's per-handle identity seam, so one in-process
test can give each role worker the user identity that separate processes or
authenticated provider clients get naturally. Operations attributed to the
current user — issue/PR creation, comments, reviews, and merges — use the
handle-local override.

`as_user` does not create a durable user directory. `get_user` still only
resolves the handle's effective current user, matching the in-memory backend's
minimal user model.

## Seeding CI jobs

The Forge interface has no CI-job creation operation. Seed jobs directly with
`MemoryForge::seed_ci_jobs(repo_id, jobs)`, which replaces any previously seeded
jobs for that repository. This mirrors `FilesystemForge::seed_ci_jobs`, which
writes the filesystem backend's `ci_jobs.json` fixture.

`MemoryForge::seed_ci_run(repo_id, pull_request_id, commit_sha)` records matching
CI presence without adding jobs. It models the provider interval after a run is
registered but before runner capacity assigns its first task, allowing missing-CI
recovery tests to distinguish that state from genuine absence.

## Fault-injection hook

Because there is no durable store to corrupt, the backend exposes a small,
deterministic fault hook so backend error paths stay testable:

- `MemoryForge::fail_next(op, message)` arms a one-shot fault. The next call to
  `op` returns `ForgeError::Backend(message)` *before* touching state; later
  calls proceed normally. Arming the same op again queues another fault.
- `MemoryForge::clear_faults()` discards every armed fault.

`op` is a `FaultOp` value. The fault-aware operations are the mutating and load
operations the workflow runtime exercises: `ListIssues`, `CreateIssue`,
`GetIssueByNumber`, `UpdateIssue`, `ListPullRequests`, `CreatePullRequest`,
`GetPullRequestByNumber`, `UpdatePullRequest`, and `MergePullRequest`. Other
`Forge` methods ignore the hook. Faults fire before any state mutation, so an
armed fault never leaves a partial change behind.

## Consistency guarantees

Each operation takes the single interior mutex for its whole duration, so
operations are atomic and serialized with respect to each other. There are no
cross-operation transactions. Use it as a single-store deterministic backend for
tests and local development.

Cross-process concurrency safety (the filesystem backend's store-level advisory
lock from ADR 0018) is **not applicable** here: a `MemoryForge` store lives in
one process's memory and cannot be shared across OS processes, and the interior
mutex already serializes every operation. The locking is a filesystem-specific
durability concern, not an observable behaviour difference, so this keeps the
ADR 0008 observable-contract parity with the filesystem backend honest.

Validation failures map to `ForgeError::InvalidRequest`, missing resources to
`ForgeError::NotFound`, duplicate repository paths to `ForgeError::AlreadyExists`,
illegal state transitions (such as merging a closed or merged pull request) to
`ForgeError::Conflict`, and armed faults to `ForgeError::Backend`. Dependency
adds require the target item number to exist as an issue or pull request in the
source repository; removals of absent links are no-ops. Review requests are
set-like and idempotent; review events are append-only and sorted by submission
time then review ID, matching the filesystem backend.

## Optimistic concurrency

`update_issue` and `update_pull_request` honour the shared `expected_version`
precondition (see ADR 0013 and the [Forge concurrency reference](forge-interface-concurrency.md)).
When `expected_version` is `Some` and does not equal the stored version, the call
returns `ForgeError::Conflict` before advancing the logical clock or mutating any
state, so a rejected compare-and-swap leaves the store untouched. When it is
`None`, the update is unconditional. The check uses the same logic and messages
as the filesystem backend. No new `FaultOp` is involved: the conditional path is
the existing `UpdateIssue`/`UpdatePullRequest` operation.
