# Forge interface reference

The Forge interface is Temper's backend-agnostic contract for collaboration
systems such as local files, Forgejo, GitHub, and test doubles.

Rust definition: `crates/temper-forge/src/forge.rs`.

This page is the entry point. Load a focused page for the contract area you
need:

- [Model and query semantics](forge-interface-model.md) — identities, lookups,
  list filters, detail levels, sorting, and error categories.
- [Core artifact operations](forge-interface-artifacts.md) — users,
  repositories, labels, issues, dependencies, and comments.
- [Pull-request, review, merge, and CI operations](forge-interface-pull-requests.md)
  — pull-request lifecycle, reviewer state, merge records, and CI reads.
- [Optimistic concurrency and idempotency](forge-interface-concurrency.md) —
  `Version`, conditional updates, create idempotency, and compatibility notes.

Backend pages document provider-specific storage, unsupported features, and
consistency limits; they must still satisfy the portable contract unless they
explicitly return `ForgeError::InvalidRequest`.

## Contract summary

The `temper_forge::Forge` trait exposes asynchronous operations for:

- current user identity and user lookup
- repositories and repository labels
- issues and pull requests
- same-repository native dependency links
- comments
- pull-request reviewer requests and review events
- pull-request merges
- CI job reads

All methods are asynchronous because remote providers are expected. Local
implementations may use synchronous internals behind the async trait. Portable
workflow logic should depend only on this interface and the portable model types.

## Companion change hints

`ChangeHint`, `ChangeKind`, `ChangeSource`, and `ChangeSourceEvent` are portable
companion types in `temper-forge`, but they are deliberately **not** methods on
the `Forge` trait. A hint source is an optional latency accelerator for runners:
it may be lossy, duplicate, stale, broad, reordered, or closed. The facade
factory exposes `new_memory_with_change_source` and
`new_filesystem_with_change_source` for hermetic composition tests that need an
abstract `Arc<dyn Forge>` plus this companion source without naming concrete
backend types.

Consumers must use hints only to wake the normal poll path, then re-read Forge
state through the trait before planning or mutating. Backends that cannot emit
hints are still complete Forge implementations; periodic polling remains the
liveness and correctness backstop.

Review, CI/status, pull-request/head, label, and push hints are broad enough to
wake mechanical landing. Multi-repository runners may narrow an immediate wake
tick to configured repositories named by known hints, but unknown/no-hint batches
and ordinary poll/audit ticks must fall back to the configured scan set.

## Quick invariants

- Store typed stable identifiers (`RepositoryId`, `IssueId`, `PullRequestId`,
  and peers) for durable synchronization; use `RepositoryPath` and `ItemNumber`
  only for display and lookup convenience.
- Read lookups return `Ok(Some(value))` when visible, `Ok(None)` when absent or
  invisible, and `Err` when the backend could not complete the lookup.
- Mutations return `ForgeError::NotFound` for missing targets and use the shared
  `ForgeError` categories in the model page.
- List results are deterministic. Use exact `get_*` lookups when branch refs,
  dependency links, reviewers, merge records, or other detail fields matter.
- Issue and pull-request updates apply label changes as `set_labels`, then
  removals, then additions; assignee changes remove before adding.
- Comment reads expose ordinary issue/PR conversation comments. The portable
  interface has no provider label-event or timeline-record operation.

## Audit retrieval boundary

Plan-validation outcomes are durable ordinary comments on the coordinating plan.
Portable callers retrieve them with `list_issue_comments`; assignment-bound
agents can use `forge_get_item(include_comments=true)`, which returns the same
conversation surface under worker response bounds. The stable
`temper:comment-key=plan-validation:<assignment-key>` marker identifies these
records. Its `assignment-sha256:<digest>` key is derived from the exact job and
attempt identity: a later validation round gets a distinct marker even when it
reuses the deterministic job ID, while exact result replay reuses the marker.
The ordinary comment renders both identifiers for retrieval and diagnosis.

Provider-specific label/timeline history is outside the current `Forge` trait.
In particular, Forgejo may persist label changes as internal timeline comment
records, but neither portable comment methods nor `forge_get_item` expose those
records. Operators and agents should inspect the validation audit comment, not
Temper journals, Forgejo SQLite, or hidden timeline storage, to determine the
validated/needs-followup outcome and its safe summary and identifiers.

## Optimistic concurrency

Issues and pull requests carry a portable `Version`. Passing
`expected_version: Some(v)` to `UpdateIssue` or `UpdatePullRequest` turns the
update into a compare-and-swap: the backend mutates only if the stored version
still equals `v`; otherwise it returns `ForgeError::Conflict` and changes
nothing. See [Optimistic concurrency and idempotency](forge-interface-concurrency.md).

## Compatibility notes

Concrete backends may expose richer provider-specific features, but portable
workflow logic should depend only on this interface. Provider label-change and
timeline records are not currently modeled; ordinary comments are not a
portable proxy for that hidden history. If a provider feature becomes broadly
useful, add it to the Forge model with documentation and backend conformance
tests.
