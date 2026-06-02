# Lesson 0023: Keep `AGENTS.md` as an orientation map

## Tags

`docs`, `process`, `agents`

## Trigger

Human steering noted that `AGENTS.md` had become too large and asked for it to
be reduced to a short preamble plus codebase/documentation map.

## What went wrong

Detailed crate status, operational rules, validation commands, backend
conventions, and session checklists accumulated in the entry-point file. That
made the first file agents read expensive to load and duplicated information
that belonged in focused Diátaxis docs.

## Steering for future agents

Keep `AGENTS.md` short. It should orient agents to the repository and point to
the right docs; it should not carry durable rules, phase status, or long crate
narratives. Put stable process rules in how-to/reference docs and update the
map only when navigation changes.

## Where this is now documented

- `AGENTS.md`
- `docs/how-to/start-a-development-session.md`
- `docs/reference/development-conventions.md`
