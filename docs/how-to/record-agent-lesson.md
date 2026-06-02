# Record an agent lesson

Use this guide when a session reveals a mistake or steering rule that future agents should know.

## 1. Decide whether it is a lesson

Record a lesson when at least one is true:

- a human corrected an assumption or design direction
- an agent made a mistake that could recur
- validation failed because docs or workflow guidance were missing
- future agents would save time by seeing the steering early

Do not record normal implementation details that belong in code, tests, or API reference.

## 2. Check for an existing lesson

Read `docs/reference/agent-lessons/README.md`. If a lesson already covers the issue, update that entry instead of creating a duplicate.

## 3. Create or update the entry

Use `docs/reference/agent-lessons/template.md` for new entries. Keep the lesson concrete:

- what triggered it
- what went wrong
- what future agents should do instead
- where the rule is now documented

## 4. Promote durable rules

If the lesson changes expected behavior, also update the canonical doc:

- `docs/how-to/start-a-development-session.md` for session flow
- `docs/reference/development-conventions.md` for development rules
- `AGENTS.md` only when the top-level orientation map changes
- reference docs for API contracts
- how-to guides for workflows
- explanation docs for rationale
- ADRs for significant decisions

## 5. Update the index

Add new entries to the active lessons table in `docs/reference/agent-lessons/README.md` with useful tags.
