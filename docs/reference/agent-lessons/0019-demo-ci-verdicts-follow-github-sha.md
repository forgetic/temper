# Lesson 0019: Demo CI verdicts follow `GITHUB_SHA`

## Tags

`examples`, `forgejo`, `ci`, `testing`

## Trigger

The reference-delivery demo stalled after Forgejo cancelled a passing PR-head CI
run, then executed an older same-branch push payload whose
`github.event.head_commit.message` lacked the `[ci-pass]` marker.

## What went wrong

The demo CI used the push event's `head_commit.message` as the verdict input.
Rapid same-branch pushes can leave Forgejo Actions judging a stale payload even
when `GITHUB_SHA` points at the newer commit the run is meant to validate.
Cancelled runs are harmless; using stale payload metadata for the non-cancelled
run is not.

## Steering for future agents

Synthetic demo CI should judge the exact commit under test (`GITHUB_SHA`). If it
needs a commit-message marker without checkout, read that commit through the
Forgejo commit API rather than trusting `github.event.head_commit.message`.

## Where this is now documented

- `examples/reference-delivery/config/ci.yml`
- `crates/harness-production/src/provision.rs`
- `crates/harness-testing/src/forgejo_server/provision.rs`
