# ADR 0006: Retire the agent lessons register

## Status

Accepted; replaces the earlier decision to maintain a standalone lessons register.

## Context

Temper originally kept short agent lessons under `docs/reference/agent-lessons/`
so human steering and repeated mistakes would not be lost between sessions. The
register later became another bootstrap document that agents were expected to
load, duplicating rules already present in canonical docs, tests, code comments,
and ADRs.

The extra memory layer worked against the documentation goal of giving agents
only task-relevant context.

## Decision

Remove the standalone agent lessons register.

Durable steering now belongs directly where future readers already look:

- development rules in `docs/reference/development-conventions.md`;
- task procedures in focused how-to guides;
- contracts in reference pages;
- rationale in explanation pages or ADRs;
- regressions in tests or local code comments.

## Consequences

Session bootstrap no longer reads a lessons index. Closeout should update the
canonical doc, test, or ADR that owns the behavior instead of adding a separate
lesson entry.
