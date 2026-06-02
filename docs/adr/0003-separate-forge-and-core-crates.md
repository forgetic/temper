# ADR 0003: Separate Forge abstractions from the workflow crate

## Status

Accepted

## Context

Temper needs both provider-neutral Forge abstractions and higher-level workflow/orchestration logic. If those concerns live in the same crate, backend contracts and workflow policy may become coupled too early.

## Decision

Place Forge domain types and the backend interface in `temper-forge`.

Reserve a separate workflow crate for workflow and orchestration logic that will be defined later. That crate is `temper-workflow` (scaffolded as `temper-core` and renamed per ADR 0007 before functionality was added). `temper-workflow` may depend on `temper-forge` when workflow logic needs Forge operations, but `temper-forge` must not depend on `temper-workflow` or concrete backends.

## Consequences

Backend implementations such as `temper-forge-filesystem` and `temper-forge-memory` depend on `temper-forge`, not the workflow crate. Workflow code can evolve independently from the provider abstraction.
