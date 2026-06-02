# ADR 0020: Support ChatGPT (OpenAI Codex) OAuth as an agent auth mode

## Status

Accepted.

## Context

`harness-agents` originally authenticated its in-process LLM agents one way: a
DeepSeek **API key** carried as the per-request bearer through the SDK's
OpenAI-compatible completions route (`src/provider.rs`). DeepSeek bills
**per token**.

The operator driving our own tests and local development pays a **flat ChatGPT
subscription** (no per-call cost). Spending DeepSeek tokens on every gated
real-agent run (the `harness-testing` Forgejo multi-process variants, the
`harness-agents` e2e, the example validation) is pure avoidable cost. The
mechanism to use a ChatGPT subscription already exists in **both** the nodejs
`pi-coding-agent` and the Rust SDK we depend on (`pi_agent_rust`): provider id
`openai-codex` routes to the Codex Responses provider, and the OAuth **access
token is the bearer** (the SDK reads the `chatgpt_account_id` JWT claim and sets
the Codex headers itself).

The login flow (`/login openai-codex`, PKCE/device-code) is already provided by
both pi CLIs. We do not want to reimplement login; we want to **consume** the
result.

## Decision

Generalize the provider seam from "one DeepSeek key" to an **auth mode**, keep
DeepSeek (`ApiKey`) as the library default, and add a `ChatGptOAuth` mode.

### Reuse the shared auth file — tolerant of its dual on-disk schema

Both pi implementations write the **same** auth file,
`pi::config::Config::auth_path()` → `~/.pi/agent/auth.json`, but in **different
on-disk schemas** for the `openai-codex` entry:

- **nodejs pi:** `{ "type": "oauth", "access", "refresh", "accountId",
  "expires" }`.
- **Rust SDK** (`AuthCredential::OAuth`, `#[serde(tag="type",
  rename_all="snake_case")]`): `{ "type": "o_auth", "access_token",
  "refresh_token", "expires", "token_url"?, "client_id"? }`.

The Rust SDK's `AuthStorage::load` will **not** deserialize a nodejs-written
entry as `OAuth`. We therefore read the file with our **own tolerant reader**
(`src/provider/oauth.rs`) that accepts both field spellings (`access` |
`access_token`, `refresh` | `refresh_token`, optional `accountId`, `expires` in
unix ms) and both `type` tags, so a login from **either** pi works. When a token
is at/near expiry we refresh it against the compiled-in OpenAI Codex token
endpoint + public client id and **write it back in the same schema we read**, so
a nodejs file stays nodejs-readable. Login itself is delegated to the pi CLIs.

### Resolve the bearer fresh per decision; never log it

The access token is short-lived, so `ChatGptOAuth` resolves the bearer (load →
refresh-if-near-expiry → access token) **each time a decision runs**, rather than
baking it into the provider object. Codex models are reasoning models, so this
mode leaves temperature unset and requests the lowest supported reasoning effort
(`low`), where the DeepSeek path pins `temperature = 0.0`. (Live validation:
the default codex model is `gpt-5.5` — ChatGPT's served model ids move over
time — and Codex rejects `minimal` effort, so `low` is the floor; both are
overridable.) No token is ever logged, formatted, or
placed in an error; failures carry only the provider/path and (for refresh) an
HTTP status.

### Selection surface and cost-driven defaults

`ProviderConfig::from_auth(choice, codex_model, auth_file)` is the selection
entry point. The codex model id and auth-file path each resolve **CLI override >
env var (`HARNESS_AGENTS_CODEX_MODEL` / `HARNESS_AGENTS_AUTH_FILE`) > built-in
default**. `from_auth` runs an **eager credential preflight** so a missing
DeepSeek key or a missing ChatGPT login fails at setup — before any worker tick —
mirroring the existing DeepSeek "key unavailable" behavior; the OAuth error
points the operator at `pi /login openai-codex`.

The `harness-testing-worker` exposes this as `--auth deepseek|chatgpt-oauth`
(later extended with `anthropic-oauth`), `--codex-model <id>`, `--auth-file
<path>`, with `HARNESS_AGENTS_AUTH` as the
config-file bridge (precedence CLI > env > default). The **library default stays
`deepseek`** (production wiring is explicit and unchanged), but the **test/dev
surfaces default to `chatgpt-oauth`** per the cost policy, so our own runs never
bill DeepSeek. DeepSeek remains fully supported as the documented fallback and as
the bring-your-own-key option for operators without a ChatGPT subscription.

## Consequences

- A ChatGPT subscriber can drive the agents with no per-call cost, reading the
  shared `auth.json` either pi CLI populates.
- The LLM SDK stays confined to `harness-agents`: the worker selects an auth mode
  by flag/value and never imports SDK auth types.
- New limits to keep visible: ChatGPT/Codex **subscription rate limits** apply,
  the OAuth access token is **short-lived** (refreshed automatically), and a
  `401`/`403` means the login expired or lacks Codex access — re-run
  `pi /login openai-codex`. The web/device login flow is **not** reimplemented
  here.
- Offline unit tests cover the dual-schema tolerant reader, schema-preserving
  write-back, the model/auth-file override resolution, and the eager preflight.
  The live path (a real decision against the Codex endpoint + a near-expiry
  refresh) is `#[ignore]`d and gated (`HARNESS_CHATGPT_OAUTH=1`), matching the
  DeepSeek e2e precedent (validated in A3).

## Alternatives considered

- **Use the Rust SDK's `AuthStorage` directly.** Rejected: it deserializes only
  the Rust schema, so a nodejs-written login (what this machine has) would not
  load. The tolerant reader accepts both.
- **Reimplement the OAuth login/PKCE flow in `harness-agents`.** Rejected: both
  pi CLIs already provide it; we consume the stored result and only refresh.
- **Default tests/dev to DeepSeek.** Rejected on cost: it bills per token for
  runs a flat subscription already covers.
