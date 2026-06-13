<!--
Optional engineer prompt OVERLAY template for the Smith coding agent.

Installed to ~/.config/anvil/prompts/engineer.md by deploy/install.sh (only if no
file is already there — your edits are never clobbered).

anvil-agent loads this when running the engineer's CodingWorkspace tool
and appends it, verbatim, as an `Operator guidance` block AFTER the built-in role
contract and BEFORE the repository AGENTS.md (see
docs/reference/coding-agent-prompts.md). It is additive only: it CANNOT remove or
rewrite the built-in contract or weaken the real-product-diff guarantee. Keep it
to house style and repo-specific implementation hints. Delete this file to run
with just the built-in prompt.

This is a deployment scaffold, not a checked-in production role prompt: it lives
on the operator's machine under ~/.config/anvil, which is exactly what the
no-checked-in-prompts rule permits (docs/reference/development-conventions.md).
-->

# Engineer implementation guidance (operator overlay)

When you implement a ready code issue for `ai/smith`:

- Produce a real product diff that satisfies the spec's acceptance criteria. Do
  not leave bookkeeping-only changes.
- Match this repo's conventions: run the same gate CI runs before you consider
  the change done — `cargo fmt --all`, `cargo dev-clippy` (warnings are errors),
  `cargo dev-check`, `cargo dev-test`.
- CI is the only landing gate here. If CI fails on your PR, push a focused fix to
  the same PR head rather than opening a new PR.
- Keep documentation in step with code, following the Diátaxis split, `AGENTS.md`,
  and `docs/reference/development-conventions.md`.
