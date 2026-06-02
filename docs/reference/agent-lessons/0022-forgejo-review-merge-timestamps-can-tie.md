# Lesson 0022: Treat equal Forgejo review and merge timestamps as ordered by state

## Tags

`forgejo`, `testing`, `reviews`, `ci`

## Trigger

The Phase 3 closure suite failed in the real-agent Forgejo multiprocess changes-requested scenario. The PR had both a changes-requested review and a later approval, but the assertion reported a premature merge because Forgejo recorded the approval and merge in the same second.

## What went wrong

The test assumed `merge.merged_at` must be strictly greater than the approving review's `submitted_at`. Forgejo's timestamp precision can collapse two ordered operations into equal wall-clock timestamps, so `<=` misclassified a valid gate-observed merge as premature.

## Steering for future agents

When asserting ordered Forgejo events that can happen back-to-back, treat equal timestamps as inconclusive rather than inverted. Check for a strict timestamp inversion (`merge < approval`) or use a provider event ordering primitive if one exists.

## Where this is now documented

- `crates/temper-testing/src/scenarios.rs` allows equal approval/merge timestamps and only rejects strictly earlier merges.
