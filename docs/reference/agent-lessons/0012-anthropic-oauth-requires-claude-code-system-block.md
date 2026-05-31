# Lesson 0012: Anthropic OAuth needs the Claude Code identity as the first system block

## Tags

`agents`, `pi-sdk`, `oauth`, `auth`, `anthropic`

## Trigger

A request to verify Anthropic OAuth (opus 4.8) end-to-end. Every decision failed
with `HTTP 429 {"type":"rate_limit_error","message":"Error"}`. The generic
`"Error"` body looked like an account rate limit, but the human knew their own
Claude opus 4.8 access worked fine and suspected the implementation.

## What went wrong

The 429 was **not** a real quota limit. Anthropic's Claude **subscription
OAuth** path rejects any `/v1/messages` request whose **first `system` block is
not exactly** `You are Claude Code, Anthropic's official CLI for Claude.` —
regardless of `anthropic-beta` flags. `harness-agents` sent only the role prompt
as `system`, so every call 429'd. Auth routing was fine (the `sk-ant-oat…` token
is correctly sent as `Authorization: Bearer`).

Empirically (live, `claude-opus-4-8`):

- role-only / arbitrary / **no** system → 429
- identity-only system (string or array) → 200
- identity prefix **appended into one string** with the role text → 429
- array with the identity as a **separate first block** + role as a second block → 200

The pinned `pi_agent_rust` 0.1.13 sends `system` as a single `Option<&str>` and
never injects the identity, so an array `system` is impossible through the SDK,
and concatenating into one string does not satisfy the check.

## Steering for future agents

- Do not treat a `429 rate_limit_error` with a bare `"message":"Error"` on the
  Anthropic OAuth path as a quota problem. It is almost always the missing
  Claude Code system identity. Reproduce with a minimal `curl` before assuming
  rate limiting.
- You need only the one identity **line**, not the full Claude Code system
  prompt.
- With the single-string-`system` SDK, send the identity as the system prompt
  and fold the role prompt into the **user** turn (what `decision.rs` now does
  via `ProviderConfig::required_system_identity`). Only Anthropic OAuth returns
  `Some`; DeepSeek and ChatGPT OAuth keep the role prompt as `system`.

## Where this is now documented

- `crates/harness-agents/src/provider/anthropic_oauth.rs`
  (`CLAUDE_CODE_SYSTEM_IDENTITY` doc comment).
- `crates/harness-agents/src/provider.rs`
  (`ProviderConfig::required_system_identity`).
- `crates/harness-agents/src/decision.rs` (`run_decision` system/user split).
- Proven by `crates/harness-agents/tests/anthropic_oauth_live.rs`
  (`HARNESS_ANTHROPIC_OAUTH=1`).
