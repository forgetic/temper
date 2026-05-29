# ADR 0003: Separate Forge abstractions from workflow core

## Status

Accepted

## Context

Harness needs both provider-neutral Forge abstractions and higher-level workflow/orchestration logic. If those concerns live in the same crate, backend contracts and workflow policy may become coupled too early.

## Decision

Place Forge domain types and the backend interface in `harness-forge`.

Reserve `harness-core` for workflow and orchestration logic that will be defined later. `harness-core` may depend on `harness-forge` when workflow logic needs Forge operations, but `harness-forge` must not depend on `harness-core` or concrete backends.

## Consequences

Backend implementations such as `harness-fs` depend on `harness-forge`, not `harness-core`. Workflow code can evolve independently from the provider abstraction.
