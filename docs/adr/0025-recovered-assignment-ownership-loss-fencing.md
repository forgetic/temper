# ADR 0025: Fence recovered work with typed durable-ownership decisions

## Status

Accepted

## Context

A daemon can reconstruct an assignment from Forge after restart. Reattachment
used to reduce every lease failure to a string, so callers could not distinguish
a temporary Forge outage from fresh evidence that another attempt owns the
artifact. The old identity predicate also omitted the attempt fence and several
assignment fields. As a result, a heartbeat could keep process-local authority
after its durable assignment or lease had disappeared.

## Decision

Recovered heartbeats return `RecoveredHeartbeatOutcome` end to end:

- `Owned` means the exact assignment and lease were conditionally refreshed;
- `TransientlyUnavailable { reason }` means ownership was not disproved, but a
  repository, transport, backend, or CAS operation prevented refresh;
- `OwnershipLost { reason }` carries a typed
  `RecoveredOwnershipLossReason` for a removed target, absent or replaced
  assignment, absent or replaced lease, or malformed fresh claim state.

`ResultApplier`, its defaults, `RoleRoutingApplier`, `LeaseApplier`, and
`LeaseManager` preserve this distinction. A recovered result is retryable only
for transient unavailability. Definitive loss makes it stale without invoking
the inner result applier.

### Exact reattachment predicate

The repository and issue/pull-request target passed to `LeaseManager` select the
only metadata record that may authorize the heartbeat. Within that record the
core identity is exact equality of:

- `job_id`;
- `attempt_id`;
- `worker_id`;
- the prior `daemon_boot_id`; and
- `role`.

When expected recovered metadata contains `queue`, `action`,
`coordination_key`, or `assignment_pr_head`, fresh metadata must contain the
same value. An expected legacy record that omits one of those later optional
fields omits only that comparison. In particular, legacy `attempt_id: None`
only equals another `None`; it is never a wildcard for `Some(new_attempt)`.
New claims continue to populate every identity field available from the job.

The lease must exist and its role and owner must match the expected assignment.
The owner is the prior daemon boot ID, falling back to worker ID only for
metadata that has no daemon boot ID. Expiry, assignment/heartbeat timestamps,
and pre-claim label or assignee restoration snapshots are deliberately outside
the identity predicate.

### Classification matrix

| Fresh observation | Outcome |
| --- | --- |
| Exact assignment and exact lease refreshed | `Owned` |
| Repository lookup, transport, or backend failure | `TransientlyUnavailable` |
| Target repository or artifact absent | `OwnershipLost::TargetRemoved` |
| Assignment absent | `OwnershipLost::AssignmentAbsent` |
| Any required assignment identity differs, including a newer attempt | `OwnershipLost::AssignmentReplaced` |
| Lease absent | `OwnershipLost::LeaseAbsent` |
| Lease role or owner differs | `OwnershipLost::LeaseReplaced` |
| Fresh metadata/claim is malformed | `OwnershipLost::MalformedClaim` |
| Conditional write is contended and one fresh read still matches | `TransientlyUnavailable` |
| Conditional write is contended and one fresh read proves a change | the corresponding `OwnershipLost` |
| Conditional write is contended and its one verification read fails | `TransientlyUnavailable` |

CAS contention alone is not evidence of loss: another heartbeat or unrelated
artifact edit may have advanced the version. `LeaseManager` therefore performs
exactly one fresh ownership read after contention. It does not retry the write
in that call.

### Process-local authority

`LeaseApplier` keys authority by exact `(job_id, attempt_id)`, not job ID alone.
It also retains a monotonic process-local revoked-attempt set. On definitive
loss it inserts the tombstone and removes authority before returning the typed
outcome. A delayed heartbeat checks the tombstone both before its Forge read and
before restoring authority, so it cannot resurrect the attempt. A different,
newer attempt for the same job has a different key. Normal and recovered result
application cannot pass through `LeaseApplier` after the exact attempt is
revoked.

## Restart boundaries

The revoked set is intentionally process-local; Forge assignment and lease
metadata remain the durable authority across restarts. After restart, a removed
or replaced target/assignment/lease fails the exact predicate and is not
reattached. A matching durable claim may be reconstructed and refreshed using
its recorded prior daemon boot identity. Temporary read failures neither clear
that claim nor create a tombstone, so a later heartbeat can recover. A
previously returned stale recovered result remains fenced by durable mismatch;
later daemon/worker cancellation and terminal-trace behavior consume this typed
outcome at their own protocol boundaries.

## Consequences

Ownership decisions are no longer inferred from log strings, legacy attempts
cannot match newer fences, and result retries are reserved for failures that do
not prove ownership loss. The stricter predicate can expose malformed or
partially restored claims that older code accepted; treating those claims as
lost is the safe behavior because execution authority cannot be proven.
