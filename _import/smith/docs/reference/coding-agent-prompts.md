# Coding-agent prompt overlays and `AGENTS.md`

The coding-workspace agent (`anvil-agent`, implemented in the sibling anvil
checkout's `crates/anvil-temper-agent/src/coding_agent.rs`) builds a hard-coded per-role
system prompt. Two optional context sources layer on top of that built-in text,
in this precedence order:

1. **Built-in role prompt** — the role contract (engineer / architect /
   reviewer). Always present, always first. Never replaced.
2. **Operator prompt overlays** — Markdown an operator drops into a config dir
   (outside the repo). Appended as an `Operator guidance` block.
3. **Repository `AGENTS.md`** — the checkout's root `./AGENTS.md`, injected as a
   `Repository AGENTS.md` block. Last, so the repo's own conventions are honored
   by default.

All three are *additive context*. The overlays and `AGENTS.md` cannot remove or
rewrite the built-in role contract; a confused or hostile overlay can only add
text, never weaken the role's verdict/diff guarantees.

> **Why this does not violate the no-checked-in-prompts rule.**
> `docs/reference/development-conventions.md` says to keep workflow-role prompts
> generated from user workflow manifests and not to add checked-in,
> role-specific *production* prompt files. That rule governs the **role-decision**
> prompts and forbids checking prompts **into this repo**. Operator overlays live
> in `~/.config/anvil` (an operator's machine), not the repo, so they are
> consistent with the rule.

## Config directory resolution

The config dir is resolved with this precedence (first match wins):

1. The `--config-dir PATH` CLI flag.
2. The `ANVIL_CONFIG_DIR` environment variable.
3. `$XDG_CONFIG_HOME/anvil` (an empty `XDG_CONFIG_HOME` is treated as unset).
4. `~/.config/anvil`.

A missing directory or any missing/blank file is a **clean no-op**: the agent
runs with just its built-in prompt. I/O errors reading an overlay are swallowed
(treated as "absent") — overlay context is best-effort and never fails a run.

## Operator overlay files

Inside the config dir, overlays live under `prompts/`:

| File | Applies to | Role |
| --- | --- | --- |
| `prompts/coding-agent.md` | every coding-agent role | shared (lower precedence) |
| `prompts/engineer.md` | `CodingWorkspace` capability | engineer |
| `prompts/architect.md` | `TriageWorkspace` capability | architect |
| `prompts/reviewer.md` | `ReviewWorkspace` capability | reviewer |

For a given run, the shared overlay (if present) is applied first, then the
single per-role overlay for that run's capability. Both are wrapped in a
delimited `=== BEGIN Operator guidance ===` … `=== END Operator guidance ===`
block so injected text cannot be confused with the built-in contract.

Example layout:

```text
~/.config/anvil/
└── prompts/
    ├── coding-agent.md   # shared house style for all roles
    ├── engineer.md       # engineer-only guidance
    ├── architect.md      # architect-only guidance
    └── reviewer.md       # reviewer-only guidance
```

## Repository `AGENTS.md`

The agent reads the checkout's root `./AGENTS.md` (relative to its cwd, which is
the prepared checkout) and injects it as a delimited
`=== BEGIN Repository AGENTS.md ===` … `=== END Repository AGENTS.md ===` block.
Only the repository root file is read for the MVP; nested-`AGENTS.md` support is
a possible follow-up.

## Anthropic-OAuth folding

Anthropic's subscription OAuth path rejects any request whose first `system`
block is not exactly the Claude Code identity (HTTP 429). In that mode the agent
already folds the role prompt into the **user turn**. The operator overlays and
`AGENTS.md` follow the same rule: under Anthropic OAuth they are appended to the
user turn, never to the first system block. All other modes keep the overlays in
the system prompt.

## See also

- [Configure provider auth](../how-to/configure-provider-auth.md)
- [Provider auth](provider-auth.md)
- [Development conventions](development-conventions.md)
