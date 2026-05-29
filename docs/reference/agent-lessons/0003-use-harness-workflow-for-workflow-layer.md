# Lesson 0003: Use `harness-workflow` for the workflow layer

## Tags

`architecture`, `crates`, `workflow`

## Trigger

Human steering clarified that the placeholder `harness-core` crate name is too vague for the planned workflow/orchestration layer.

## What went wrong

The existing name suggested a generic foundation crate instead of the specific layer responsible for workflow specifications, validation, compilation, runtime transitions, and recovery.

## Steering for future agents

Rename `harness-core` to `harness-workflow` before adding workflow functionality. Keep Forge abstractions in `harness-forge`; put workflow policy and orchestration in `harness-workflow`; keep agent-provider execution outside both layers until it is deliberately designed.

## Where this is now documented

- `docs/adr/0007-workflow-layer-and-agent-compilation.md`
- `docs/reference/workflow-layer.md`
- `docs/how-to/implement-workflow-layer-in-phases.md`
