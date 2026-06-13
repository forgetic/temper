# ADR 0003: Support ChatGPT/OpenAI Codex OAuth as an agent auth mode

## Status

Accepted.

## Context

Smith supports real LLM responders for local development and gated e2e tests.
DeepSeek API-key auth remains useful, but it bills per token. Operators with a
flat ChatGPT subscription can use the pi `openai-codex` provider route instead.
Both pi CLIs already implement the OAuth login flow, so Smith should consume the
shared auth file rather than reimplement login.

## Decision

Smith supports an explicit `chatgpt-oauth` auth mode alongside `deepseek` and
`anthropic-oauth`.

### Reuse the shared auth file tolerantly

Both pi implementations write `~/.pi/agent/auth.json`, but the `openai-codex`
entry has two schemas:

- nodejs pi: `{ "type": "oauth", "access", "refresh", "accountId", "expires" }`;
- Rust pi SDK: `{ "type": "o_auth", "access_token", "refresh_token", "expires" }`.

Smith reads both spellings and both type tags. When refreshing a near-expiry
token, it writes back in the same schema it read and preserves unknown fields, so
a nodejs-written file stays nodejs-readable.

### Resolve the bearer per decision

The Codex bearer is the OAuth access token. Smith resolves it fresh for each LLM
decision and refreshes it when near expiry. Tokens are never logged, formatted,
or included in errors; failures carry only provider/path/status information.

Codex models are reasoning models. Smith leaves temperature unset and requests
low reasoning effort, which live validation found to be the lowest supported
Codex effort for the served model.

### Selection surface

Responder binaries accept `--auth chatgpt-oauth`, `--codex-model MODEL`, and
`--auth-file PATH`. Overrides resolve as CLI > env > default:

- `TEMPER_AGENTS_CODEX_MODEL` or default `gpt-5.5`;
- `TEMPER_AGENTS_AUTH_FILE` or `~/.pi/agent/auth.json`.

Preflight checks fail early when the OAuth entry is missing and point the
operator at `pi /login openai-codex`.

## Consequences

- ChatGPT subscribers can run Smith responders without DeepSeek per-token cost.
- Smith, not Temper, owns OAuth schema tolerance and refresh behavior.
- Subscription rate limits and access eligibility still apply; `401`/`403`
  usually means re-run `pi /login openai-codex` or choose another provider.
- Live refresh tests may rotate the refresh token and must write the refreshed
  credential back to the real auth file.

## Alternatives considered

- Use the Rust SDK auth storage directly. Rejected because it does not load the
  nodejs-written OAuth schema.
- Reimplement OAuth login. Rejected because both pi CLIs already provide login.
- Default all local runs to DeepSeek. Rejected on cost grounds.
