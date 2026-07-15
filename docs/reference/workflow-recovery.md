# Workflow recovery

This page records the workflow-layer recovery primitives: leases, the command
journal, reconciliation, and application of recovery actions. For tested safety
properties, read [robustness guarantees](robustness-guarantees.md).

## Claims and leases

A claim is a lease stored in workflow metadata, not permanent ownership.
`metadata::Lease` records role, worker id, claim time, heartbeat time, and
expiration time. A lease is expired when `now >= expires_at`.

The adjacent `assignment` record is the durable dispatch identity. It is
committed in the same conditional body update as the claim, before `Assign` is
published, and records `job_id`, role, queue/action, worker id, coordination key,
daemon boot id, assignment PR head, pre-claim labels/assignees, and assignment
and expiry timestamps. A daemon restart inventories these records while its
recovery barrier is closed. Only a heartbeat from the recorded worker naming the
exact job can reattach and refresh it; unknown jobs, another worker, or another
boot identity cannot extend the lease. At the end of the bounded grace period,
unattached records are detached from the dispatch core while the barrier remains
closed. Recovery then reloads every artifact from Forge: issues with unresolved
prerequisites return to `blocked`, otherwise they return to their recorded queue;
PRs whose head advanced publish the assigned repair transition and
`repaired_head` atomically instead of restoring the old repair label. Malformed
or ambiguous records are cleared when parseable, labelled `needs-human`, and
receive one idempotent audit comment rather than being guessed.

`LeasePlanner` is pure. It:

- grants acquisition when no lease exists or the existing lease expired;
- refreshes acquisition by the current holder without changing `claimed_at`;
- rejects acquisition by a peer holding a live lease;
- heartbeats only for the current holder;
- releases idempotently for the holder or an empty lease.

`LeaseManager` applies these decisions by rewriting the metadata block through a
conditional body update. It captures the load-time `Version`, so reference
backends enforce compare-and-swap lease acquisition. Clearing another worker's
lease is a reconciler authority path, not a peer release.

## Startup recovery barrier

Startup closes dispatch before inventory. Recovery first completes durable
child-create intents, then stages valid assignments for heartbeat reattachment,
converges expired/impossible/orphaned claims, and runs one bounded mechanical
reconciliation pass. Only after every Forge mutation succeeds does it release
deferred enqueues and long-poll waiters and start normal role feeds. A convergence
failure therefore leaves the barrier closed. This ordering prevents a partially
wired child or a second session for an assigned source from escaping the restart
window in either split or standalone wiring.

Child fan-out persists a normalized intent on the parent before creating any
child. A new intent takes a known-first path with no correlation-history scan;
recovery groups unresolved keys by repository and issues one open/closed summary
query pair per repository. Children are created atomically with final labels and
`staged: true`. Returned child numbers are checkpointed once after creation,
each dependent child receives its complete sorted dependency list in one update,
and all child references/wiring progress are aggregated in one parent update.
Only then does activation clear `staged`. The final source update atomically
records activation/completion progress and the routed transition. Retries resume
the latest matching incomplete round, while a later execution whose payload or
current source completion differs receives a new durable round and round-scoped
child correlation keys. A retry therefore neither duplicates a child nor
dispatches a partially wired child, and a later legitimate fan-out cannot alias
or discard an earlier round's children.

## Workspace and repaired-PR convergence

A reusable writable checkout is inspected before reset or fetch. Local commits,
tracked edits, and untracked files are preserved under deterministic recovery
refs/stashes and replayed over the current remote target. If replay cannot be
proved safe, the checkout is moved to one actionable quarantine with a manifest
and recovery commands; retries reuse that quarantine instead of overwriting it.

PR repair records `repaired_head` when the repaired branch is published. Landing
continues to require CI for that exact head: missing, queued, running, or stale
pre-repair CI cannot land it. Once current-head CI succeeds, the normal
mechanical path converges without requesting a repeated repair.

## Command journal

`CommandJournal` records the lifecycle of a runtime command:

```text
Planned -> Applying -> Completed | Failed | Reconciled
```

`Planned` and `Applying` are incomplete states. A `CommandRecord` includes the
caller-chosen `CommandId`, target, transition, role, planned effects, detail,
and timestamps. The trait is async and supports idempotent append, state
transition, get, list, and `incomplete`.

`Executor::execute_journaled` previews the plan, records `Planned`, advances to
`Applying` immediately before mutation, and finishes at `Completed` or `Failed`.
If the process stops after mutation but before a terminal journal update, the
next reconciler scan can repair or mark it reconciled.

## Reconciliation

`Reconciler::scan` is pure and deterministic over explicit artifact snapshots,
command records, dependency status, recovery policy, and `now`. The bounded
runtime `reconcile` path loads exact incomplete-journal targets and
workflow-labelled candidates; `reconcile_deep_audit` is the deliberate
all-history operator path.

Findings include:

- `ExpiredLease`;
- `ImpossibleState` and other classification drift;
- `BlockedWithoutDependencies`;
- `PartialTransition`;
- `StaleCommand`;
- `DependenciesResolved`.

Each finding receives one action: `RequeueLease`, `Escalate`, `Repair`,
`MarkReconciled`, `Unblock`, or `Diagnose`. The default policy requeues expired
leases, escalates impossible state/drift, diagnoses dependency-gated work with no
dependencies, repairs partial transitions, marks stale commands reconciled, and
mechanically unblocks dependency-gated work once every prerequisite has landed.

Scan order is stable: per snapshot, lease and classification/dependency findings
in snapshot order, then incomplete journal commands in journal order. Child repo
read failures do not abort a scan; unreadable dependency targets remain not
landed so they cannot produce a false unblock.

## Applying reconciler actions

`recover::Applier` routes each action through the component that owns the
matching mutation:

- `RequeueLease` -> `LeaseManager::clear`;
- `Repair` -> executor label-effect application, then journal `Reconciled`;
- `Unblock` -> executor label-effect application with a fresh journaled command;
- `MarkReconciled` -> journal state transition;
- `Escalate` / `Diagnose` -> advisory output only.

Every mutating action loads fresh state and applies at most once. Re-running the
same report is a no-op rather than a double-apply, and running scan -> apply to a
fixpoint converges.
