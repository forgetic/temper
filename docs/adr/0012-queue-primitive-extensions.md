# ADR 0012: Extend queues with activation and richer matching

## Status

Accepted

## Context

The reference delivery workflow needs two queue capabilities beyond the original
single-kind, AND-label filter. First, the owner should review landed work in
cohorts: `owner_alignment` should wait until enough pending PRs accumulate, but
still fire after a quiet period so slow weeks do not starve. Second, Phase 14
will remove fixture workarounds for queues that conceptually span multiple
artifact kinds or match one of several label sets.

These are workflow read-side concerns. They decide what work a runner should
service; they do not change Forge state, executor semantics, leases, journals,
or reconciliation.

## Decision

Add optional activation fields to the queue primitive:

- `min_depth`: service the queue once matched member count reaches this number.
- `max_age`: service the queue once the oldest matched member is at least this
  many seconds old.

The planner exposes a pure predicate `queue_active(queue, members, now)`. A
queue with no activation policy remains active whenever it has at least one
matched member. A policy queue is active when `depth >= min_depth` **or** the
oldest timestamped member reaches `max_age`; empty queues are never active.
Existing queue matching stays unchanged.

Member age is measured from the classified artifact's portable Forge
`updated_at` timestamp. Workflow label updates move that timestamp when an item
enters a queue; if a caller classifies from snapshots without timestamps, only
`min_depth` can activate that queue.

Phase 14 should extend the same queue primitive instead of adding a second
routing object:

- multi-kind queues should let one queue select more than one artifact kind;
- disjunctive matching should let a queue match any of several AND-label
  clauses while preserving the current `labels` field as the single-clause
  shorthand.

## Consequences

- Batched roles can be driven by deterministic queue state instead of schedules
  or role prose.
- The executor, lease manager, journal, and reconciler do not change.
- Runtime queue manifests carry activation policy alongside subscribers.
- Phase 14 has one ADR to reference for the remaining queue extensions.

## Alternatives considered

Use a cron interval for owner alignment. Rejected because it over-fires when no
work is pending and under-fires when a cohort forms immediately after a run.

Encode batching in prompts. Rejected because servicing policy belongs in the
planner, where it is testable and visible to all runners.
