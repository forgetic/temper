# ADR 0004: Keep documentation files small

## Status

Accepted

## Context

Agents build task context by reading documentation. Large catch-all documents make sessions slower and increase the chance that irrelevant context displaces relevant details.

## Decision

Keep hand-written documentation focused. Aim for about 150 lines or fewer per file, and split documents before they exceed about 350 lines.

Use short index pages to point readers to focused tutorials, how-to guides, reference pages, explanations, and ADRs.

## Consequences

Contributors should create new focused pages instead of expanding broad documents indefinitely. Session closeout includes checking that changed documentation remains within the size budget.
