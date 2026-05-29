# ADR 0003: Separate Forge abstractions from the workflow crate

## Status

Accepted

## Context

Harness needs both provider-neutral Forge abstractions and higher-level workflow/orchestration logic. If those concerns live in the same crate, backend contracts and workflow policy may become coupled too early.

## Decision

Place Forge domain types and the backend interface in `harness-forge`.

Reserve a separate workflow crate for workflow and orchestration logic that will be defined later. That crate is `harness-workflow` (scaffolded as `harness-core` and renamed per ADR 0007 before functionality was added). `harness-workflow` may depend on `harness-forge` when workflow logic needs Forge operations, but `harness-forge` must not depend on `harness-workflow` or concrete backends.

## Consequences

Backend implementations such as `harness-forge-filesystem` and `harness-forge-memory` depend on `harness-forge`, not the workflow crate. Workflow code can evolve independently from the provider abstraction.
