# Lesson 0026: Answer questions before implementation

## Tags

`process`, `communication`, `testing`

## Trigger

A human proposed a broad testing-policy change and ended with "Any question?".
The agent started editing before answering or asking clarifying questions, then
had to pause and recover direction.

## What went wrong

The prompt was explicitly asking for questions, not immediate implementation.
Jumping straight into code risked encoding assumptions about default tests,
Forgejo cache population, and which ignored tests should run before handoff.

## Steering for future agents

When a human asks whether there are questions, answer that first. For broad
process or testing changes, clarify scope and trade-offs before editing. Start
implementation only after the human confirms the direction or explicitly asks to
proceed.

## Where this is now documented

- `docs/how-to/start-a-development-session.md` (clarify open questions before editing).
- This lesson records the correction.
