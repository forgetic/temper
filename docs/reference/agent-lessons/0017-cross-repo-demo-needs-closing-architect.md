# Lesson 0017: Cross-repo demo needs the closing architect

## Tags

`examples`, `cross-repo`, `workflow`, `agents`, `forgejo`

## Trigger

The reference-delivery demo reached merged PRs but left child code issues open
with `in-progress`, and the parent stayed `blocked` instead of unblocking and
getting follow-up work.

## What went wrong

The production worker used the default real architect, which clears `landed` on
merged PRs but does not close the produced parent code issue. Cross-repo
dependency aggregation treats issue closure as the portable "landed" signal for
issue dependencies, so open child issues keep the parent blocked forever.

## Steering for future agents

When a demo or test expects dependency unblocking through produced code issues,
ensure that issue closure is modeled or supplied by an explicit test fixture. Do
not assume merging a PR automatically closes its parent issue. The Temper
reference-delivery launcher now uses deterministic fake agents and sets the
closing fake architect by default for this reason; Smith-backed production-style
responders must model the same behavior through configuration or tools.

## Where this is now documented

- `examples/reference-delivery/config/temper.env`
- `examples/reference-delivery/README.md`
- `docs/how-to/run-cross-repo-reference-delivery-demo.md`
