# Lesson 0001: Keep Forge abstractions out of the workflow crate

## Tags

`architecture`, `crates`, `forge`

## Trigger

The initial scaffold put the Forge domain model and `Forge` trait in `harness-core`. Human steering clarified that `harness-core` should be reserved for workflow and orchestration logic to be defined later.

## What went wrong

The crate name `core` was treated as a general home for foundational abstractions. That coupled provider contracts to the future workflow layer too early.

## Steering for future agents

Put provider-neutral Forge types and traits in `harness-forge`. Backend crates such as `harness-fs` should depend on `harness-forge`, not the workflow crate.

Only add dependencies from `harness-workflow` to `harness-forge` when implementing actual workflow/orchestration logic that needs Forge operations. The placeholder crate was originally named `harness-core`; per ADR 0007 it has been renamed to `harness-workflow`.

## Where this is now documented

- `README.md`
- `AGENTS.md`
- `docs/adr/0003-separate-forge-and-core-crates.md`
- `docs/adr/0007-workflow-layer-and-agent-compilation.md`
- `docs/reference/forge-interface.md`
