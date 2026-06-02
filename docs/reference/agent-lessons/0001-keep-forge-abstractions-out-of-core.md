# Lesson 0001: Keep Forge abstractions out of the workflow crate

## Tags

`architecture`, `crates`, `forge`

## Trigger

The initial scaffold put the Forge domain model and `Forge` trait in `temper-core`. Human steering clarified that `temper-core` should be reserved for workflow and orchestration logic to be defined later.

## What went wrong

The crate name `core` was treated as a general home for foundational abstractions. That coupled provider contracts to the future workflow layer too early.

## Steering for future agents

Put provider-neutral Forge types and traits in `temper-forge`. Backend crates such as `temper-forge-filesystem` and `temper-forge-memory` should depend on `temper-forge`, not the workflow crate.

Only add dependencies from `temper-workflow` to `temper-forge` when implementing actual workflow/orchestration logic that needs Forge operations. The placeholder crate was originally named `temper-core`; per ADR 0007 it has been renamed to `temper-workflow`.

## Where this is now documented

- `README.md`
- `docs/reference/development-conventions.md`
- `docs/adr/0003-separate-forge-and-core-crates.md`
- `docs/adr/0007-workflow-layer-and-agent-compilation.md`
- `docs/reference/forge-interface.md`
