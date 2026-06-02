# ADR 0005: Optimize defaults for fast local iteration

## Status

Accepted

## Context

Temper is currently in early design and implementation. Agents need short feedback loops more than optimized production artifacts.

## Decision

Make local development defaults favor compilation speed:

- use `cargo check` as the default validation loop
- provide `cargo dev-check` and related aliases in `.cargo/config.toml`
- keep Cargo's default parallelism so all logical CPU cores are used
- tune dev/test profiles for low optimization, reduced debug info, incremental compilation, many codegen units, and no LTO

Production build settings will be designed later when release requirements are known.

## Consequences

Default local builds should be quick and suitable for interactive agent work. Debuggability of dependency internals and runtime performance are secondary during this phase.
