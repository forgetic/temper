# Development conventions

This page holds stable contribution rules for Smith.

## General rules

- Prefer small, documented changes.
- Add or update tests for behavior changes.
- Keep docs in the same session as code changes.
- Never commit provider credentials, copied auth files, tokens, or `.env` files.

## Crate boundaries

- Keep concrete LLM SDK usage inside Smith.
- Smith may depend on Temper protocol/domain crates by local path; Temper must
  not depend on Smith as a Rust crate.
- Smith responders do not receive Forge handles, Forge tokens, broad bash/file
  tools, or workflow mutation tools.
- Workflow authority stays in Temper: Smith chooses an authorized action or
  returns inert proposals; Temper validates and mutates state.
- Keep product-manager behavior an interactive profile, not a workflow role.
- Keep workflow-role prompts generated from user workflow manifests; do not add
  checked-in role-specific production prompt files.

## Rust conventions

- Use typed identifiers from Temper crates where protocol structs provide them.
- Prefer explicit state enums over stringly typed statuses.
- Avoid global mutable state.
- Keep Rust source and test files at or below about 600 lines; split focused
  modules or shared test support before exceeding that budget.

## Documentation conventions

- Follow Diátaxis: tutorials teach, how-to guides solve tasks, reference pages
  define contracts, explanation pages give rationale, and ADRs record decisions.
- Keep hand-written docs focused: aim for about 150 lines or fewer and split
  before about 350 lines.
- Capture recurring mistakes or human steering in `docs/reference/agent-lessons/`
  and promote durable rules to canonical docs.

## Workflow

See:

`docs/how-to/fast-local-iteration.md`,
`docs/how-to/end-a-development-session.md`, and
