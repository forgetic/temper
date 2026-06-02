# Lesson 0003: Use `temper-workflow` for the workflow layer

## Tags

`architecture`, `crates`, `workflow`

## Trigger

Human steering clarified that the placeholder `temper-core` crate name is too vague for the planned workflow/orchestration layer.

## What went wrong

The existing name suggested a generic foundation crate instead of the specific layer responsible for workflow specifications, validation, compilation, runtime transitions, and recovery.

## Steering for future agents

The placeholder `temper-core` was renamed to `temper-workflow` before adding any workflow functionality. Keep Forge abstractions in `temper-forge`; put workflow policy and orchestration in `temper-workflow`; keep agent-provider execution outside both layers until it is deliberately designed.

## Where this is now documented

- `docs/adr/0007-workflow-layer-and-agent-compilation.md`
- `docs/reference/workflow-layer.md`
- `docs/how-to/implement-workflow-layer-in-phases.md`
