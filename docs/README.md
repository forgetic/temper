# Documentation

Temper documentation follows Diátaxis so that each document has a clear job and agents can load only the context relevant to their task.

## Tutorials

Learning-oriented, step-by-step introductions. Use these when the reader wants to acquire the basic skill of using or extending Temper.

See `tutorials/`.

## How-to guides

Task-oriented recipes. Use these when the reader already understands the basics and wants to accomplish one specific task.

Start with:

- `how-to/start-a-development-session.md`
- `how-to/fast-local-iteration.md`
- `how-to/end-a-development-session.md`

See `how-to/` for the full index.

## Reference

Precise contracts and factual details. Use these to define APIs, invariants, schemas, compatibility requirements, and the agent lessons register.

Start with `reference/development-conventions.md` and `reference/agent-lessons/README.md` during session bootstrap, then read task-relevant reference pages.

See `reference/`, including the [interactive conversation interface](reference/interactive-conversation-interface.md).

## Explanation

Conceptual material, rationale, and trade-offs. Use these to explain why the system is shaped the way it is.

See `explanation/`, including the [interactive agent interfaces](explanation/interactive-agent-interfaces.md) and [runner process split bridge](explanation/runner-process-split.md).

## Document size budget

Keep hand-written docs focused: aim for about 150 lines or fewer, and split a page before it exceeds about 350 lines. If a page grows too large, turn it into a short index and move details into task-specific pages.

## Architecture decision records

Decision records capture significant choices that should remain visible to future agents.

See `adr/`.
