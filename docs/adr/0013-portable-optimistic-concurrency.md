# ADR 0013: Portable optimistic concurrency for conditional artifact writes

## Status

Accepted

## Context

Lease acquisition had a lost-update window. `LeaseManager::acquire` (see
`crates/harness-workflow/src/lease.rs`) loads the artifact, sees "no lease",
plans a grant, then writes the lease into the metadata block with an
unconditional body update. Two workers that both read "no lease" before either
writes both grant themselves the lease; last write wins, and two workers believe
they hold an exclusive claim. The metadata holds one lease, so two *recorded*
leases are impossible, but the *decision* was not atomic. This was the documented
top limitation in `docs/reference/robustness-guarantees.md`.

The runtime is pull-based and re-reads fresh state before every mutation, so most
races are harmless. But the read-then-write gap in `LeaseManager` is a genuine
lost update, and the webhook-accelerated triggering model (ADR 0009) widens the
window: a faster trigger makes two workers more likely to react to the same
artifact at the same instant. Closing it requires an atomic compare-and-swap /
conditional-update primitive on the portable `Forge` interface, which it did not
expose.

`Issue`/`PullRequest` already carry `updated_at`, but `UpdateIssue`/
`UpdatePullRequest` are partial updates with no precondition.

## Decision

Add **optimistic concurrency** to the `Forge` interface as a portable conditional
write, modelled as a row-version / ETag precondition rather than a new method:

1. **A dedicated monotonic `Version` token on `Issue` and `PullRequest`.**
   `harness_forge::Version` is an opaque `u64` newtype. A backend assigns
   `Version::INITIAL` on create and advances it on **every** successful mutation
   of the artifact record (`update_*` and `merge_pull_request`). Adding a comment
   does not change the artifact record, so it does not advance the version.

2. **An `expected_version: Option<Version>` precondition on `UpdateIssue` and
   `UpdatePullRequest`.** When `Some(v)`, the update is a compare-and-swap: it
   applies only if the stored version equals `v`, and otherwise returns
   `ForgeError::Conflict` **without mutating anything** (and, in the reference
   backends, without advancing the logical clock). When `None`, the update is
   unconditional — the prior behaviour, kept for backward compatibility.

### Why a version token, not a timestamp

We deliberately use a dedicated counter, not `updated_at`. Reusing a timestamp
collides whenever two mutations share a clock value: the reference backends
advance their logical clock by a whole second per write, and a real forge's
`updated_at` has finite resolution, so two writes in the same tick would carry
equal timestamps and a CAS keyed on them would silently admit a lost update. A
counter advances on every write, so no two successive versions ever coincide.

### Why a precondition field, not a new method

A precondition on the existing partial-update methods keeps the mutation surface
small and is the standard optimistic-concurrency shape. A dedicated
`compare_and_swap_*` method would duplicate the whole update surface and still
need the version token on the read model. The precondition is also exactly what a
real forge can satisfy: map `expected_version` onto an HTTP `If-Match: <etag>`
header (the artifact's version becomes its ETag), returning `412 Precondition
Failed` → `ForgeError::Conflict`. A backend without ETags can approximate it with
a single atomic claim (a conditional label/assignee set, or a database row
version). None of these leak into the trait: callers only ever see `Version` and
`ForgeError::Conflict`.

### What this does not do

This is not triggering (ADR 0009 keeps that off the trait) and not a transaction
across artifacts: it is a single-record conditional write. The executor
(`execute::Executor`) keeps writing unconditionally (`expected_version: None`),
because it re-loads fresh state and verifies postconditions on every transition;
it is not the lost-update target. Leases are.

## Consequences

- `LeaseManager` captures the version at load time and writes the lease
  conditionally on it. `acquire`, `heartbeat`, and `release` all become
  compare-and-swap, so a peer that stole an expired lease, the reconciler
  clearing one, or a racing acquirer all move the version and cause a stale write
  to be **refused** rather than clobber. The loser observes `LeaseError::Contended`
  (a lost CAS), distinct from `LeaseError::Conflict` (the planner refusing a live
  lease). `LeaseManager::prepare_acquire` + `commit` expose the load/plan and the
  conditional write as separate steps so callers and tests can interleave two
  acquirers explicitly.
- Two concurrent "no-lease" acquirers can no longer both win: the first commit
  advances the version, the second fails its CAS. Proven by
  `two_no_lease_acquirers_cannot_both_win_the_same_claim`
  (`tests/safety_properties.rs`) and
  `interleaved_acquirers_cannot_both_win_the_same_unclaimed_issue`
  (`tests/leases.rs`), each capturing the load-time token instead of hand-ordering
  the writes, plus backend conditional-write tests in both reference backends.
- Both reference backends implement identical observable semantics (ADR 0008):
  initial version on create, version advance on every record mutation, and
  `ForgeError::Conflict` on a precondition mismatch checked before any state or
  clock change. No new `FaultOp` is needed — the conditional path is the existing
  `update_issue`/`update_pull_request` operation.
- The change is backward compatible: `expected_version` defaults to `None`, the
  `version` field is `#[serde(default)]` (a pre-versioning record reads as
  `Version::INITIAL`), and every existing `Update*` literal uses `..default()`.

## Follow-up work

- Apply the reconciler's recovery actions (including the dependency `Unblock`)
  through the executor and lease manager, which remains decided-but-not-applied.
- Consider whether the executor should adopt conditional writes for specific
  transitions where a re-plan is not sufficient.
