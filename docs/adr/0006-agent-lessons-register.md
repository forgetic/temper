# ADR 0006: Maintain an agent lessons register

## Status

Accepted

## Context

Temper is intended to be developed by autonomous agents over many sessions. Human steering and agent mistakes can be lost if they only appear in chat history.

Future agents need a compact way to discover past lessons without loading every document or repeating known mistakes.

## Decision

Maintain a focused lessons register under `docs/reference/agent-lessons/`.

Each lesson records:

- trigger
- what went wrong
- steering for future agents
- where the rule is now documented

The register index is read during session bootstrap. New lessons are added during session closeout when a correction, failed assumption, or missing workflow guidance should inform future sessions.

## Consequences

Lessons remain separate from ADRs: ADRs record decisions, while lessons record learning from mistakes and steering. Durable rules discovered through lessons must also be promoted to the canonical docs that agents already read.
