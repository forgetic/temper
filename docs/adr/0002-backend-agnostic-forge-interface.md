# ADR 0002: Define a backend-agnostic Forge interface

## Status

Accepted

## Context

Temper workflows should run against Forgejo, GitHub, local files, and future Forge-like systems. Binding workflow logic directly to one provider would make agents harder to test and harder to migrate.

## Decision

Define the collaboration contract in `temper-forge` as a provider-neutral async Rust trait named `Forge`. Concrete backends implement this trait and translate provider-specific behavior into portable domain types and error categories.

Reserve a separate workflow crate for workflow and orchestration logic built on top of Forge abstractions. ADR 0007 names that crate `temper-workflow`.

The first concrete backend is a filesystem backend for deterministic local development and tests.

## Consequences

Provider-specific capabilities must not leak into workflow logic by default. When a provider feature is important enough to become portable, it should be added deliberately to the Forge model, reference documentation, and backend conformance tests.
