# Lesson 0002: Keep source files under 600 lines

## Tags

`workflow`, `rust`, `maintainability`

## Trigger

Human steering added a development rule that source files should not exceed 600 lines of code.

## What went wrong

The filesystem backend implementation and integration tests had grown into large files, making it harder for agents to load, review, and modify focused areas safely.

## Steering for future agents

Keep Rust source and test files at or below 600 lines. Split large implementations into focused modules and move duplicated test setup into shared test support before files exceed the budget.

Check with:

```sh
find crates -type f -name '*.rs' -print0 | xargs -0 wc -l | sort -n
```

## Where this is now documented

- `docs/reference/development-conventions.md`
- `docs/how-to/end-a-development-session.md`
