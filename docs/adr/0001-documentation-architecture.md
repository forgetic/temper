# ADR 0001: Use Diátaxis for documentation

## Status

Accepted

## Context

Temper is intended to be developed over time by humans and autonomous agents. Future contributors need documentation that makes intent, tasks, contracts, and rationale easy to find.

## Decision

Organize documentation with Diátaxis:

- tutorials for learning
- how-to guides for tasks
- reference for exact contracts
- explanation for rationale

Architecture decision records live in `docs/adr/` as a supporting record of significant decisions.

## Consequences

Contributors must place documentation according to reader need rather than dumping all information into one guide. Public API and domain-model changes should update reference and explanation documents when applicable.
