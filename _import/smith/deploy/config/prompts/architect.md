<!--
Optional architect prompt OVERLAY template for the Smith coding agent.

Installed to ~/.config/anvil/prompts/architect.md by deploy/install.sh (only if
no file is already there — your edits are never clobbered).

anvil-agent loads this when running the architect's TriageWorkspace tool
and appends it, verbatim, as an `Operator guidance` block AFTER the built-in role
contract and BEFORE the repository AGENTS.md (see
docs/reference/coding-agent-prompts.md). It is additive only: it CANNOT remove or
rewrite the built-in contract or weaken the ready_code verdict guarantee. Keep it
to house style and repo-specific triage hints. Delete this file to run with just
the built-in prompt.

This is a deployment scaffold, not a checked-in production role prompt: it lives
on the operator's machine under ~/.config/anvil, which is exactly what the
no-checked-in-prompts rule permits (docs/reference/development-conventions.md).
-->

# Architect triage guidance (operator overlay)

When you rewrite a thin intake into a ready code spec for `ai/smith`:

- Read the actual repository before designing. Reference real files, crates, and
  conventions — do not invent module paths.
- Make the rewritten body self-contained for the engineer: name the user-facing
  interface, define the default behavior, list the file(s) to touch, and give
  explicit, checkable acceptance criteria.
- Keep the scope to one converging change. Prefer the smallest spec that fully
  satisfies the filer's intent over a speculative redesign.
- Honor this repo's Diátaxis docs split and the conventions in `AGENTS.md` and
  `docs/reference/development-conventions.md`.
