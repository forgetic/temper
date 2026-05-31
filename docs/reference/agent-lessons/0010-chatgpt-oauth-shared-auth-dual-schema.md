# Lesson 0010: Read the shared pi auth.json tolerantly (dual on-disk schema)

## Tags

`agents`, `pi-sdk`, `oauth`, `auth`, `forgejo`

## Trigger

Wiring ChatGPT (OpenAI Codex) OAuth auth into `harness-agents` (plan
`real-world-example`, Track A). The plan is to reuse the shared
`~/.pi/agent/auth.json` that both pi CLIs write and let the Rust SDK load it.

## What went wrong

The nodejs `pi-coding-agent` and the Rust SDK (`pi_agent_rust`) write the **same
file** (`pi::config::Config::auth_path()`) but the `openai-codex` entry in
**different on-disk schemas**:

- nodejs: `{ "type": "oauth", "access", "refresh", "accountId", "expires" }`
- Rust SDK (`AuthCredential::OAuth`, `#[serde(tag="type",
  rename_all="snake_case")]`): `{ "type": "o_auth", "access_token",
  "refresh_token", "expires", ... }`

So `AuthStorage::load` will **not** deserialize a nodejs-written entry as
`OAuth`. Assuming the SDK serde parses whatever login exists is wrong, and the
login present on this machine is the nodejs one.

Two more details that bite: the bearer the Codex route wants is the OAuth
**access token** (a JWT; the SDK reads its `chatgpt_account_id` claim itself, so
the bearer alone suffices — `accountId` is informational), and `expires` is in
**unix milliseconds**.

## Steering for future agents

- Read `~/.pi/agent/auth.json` with a **tolerant reader** that accepts both field
  spellings (`access`|`access_token`, `refresh`|`refresh_token`) and both `type`
  tags (`oauth`|`o_auth`), so a login from either pi works.
- When refreshing a near-expiry token, write it back in the **same schema you
  read** (preserve `accountId` and other unknown fields), so a nodejs file stays
  nodejs-readable.
- Supply only the access-token bearer for the `openai-codex` route; do not try to
  set `chatgpt-account-id` yourself.
- Never log/format the token; errors carry only the path/provider/HTTP status.
- Delegate login (`pi /login openai-codex`) to the pi CLIs; do not reimplement
  the browser/device flow.
- **Refresh rotates the refresh token.** A3 live validation confirmed the Codex
  token endpoint returns a *new* `refresh_token`, so the old one is invalidated
  on use. Any test that forces a refresh on a **copy** of the real auth file must
  sync the refreshed credential back to the real file (the write-back preserves
  schema), or the next run finds a stale refresh token. Do not point a throwaway
  refresh test at a copy and then discard it.

## Where this is now documented

- `crates/harness-agents/src/provider/oauth.rs` (tolerant reader + write-back).
- ADR 0020 (`docs/adr/0020-chatgpt-oauth-agent-auth.md`).
- `docs/how-to/use-chatgpt-oauth-auth.md`.
