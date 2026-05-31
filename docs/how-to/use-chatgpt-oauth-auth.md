# Use ChatGPT (OpenAI Codex) OAuth for the LLM agents

The real, in-process LLM agents (`harness-agents`) can authenticate two ways: a
DeepSeek **API key** (pay-per-token) or a ChatGPT (OpenAI Codex) **OAuth
subscription** (a flat subscription, no per-call cost). This guide covers the
OAuth mode. See ADR 0020 for the rationale.

> **Cost note.** Prefer ChatGPT OAuth for tests and local development: it bills
> nothing per call against a flat subscription, where DeepSeek bills per token.
> The worker and the gated real-agent tests default to ChatGPT OAuth for this
> reason; DeepSeek stays the documented fallback for operators without a
> subscription.

## 1. Log in once (populates the shared auth file)

The agents **consume** a login; they do not perform it. Run one of the pi CLIs
once to populate the shared auth file (`~/.pi/agent/auth.json`):

```sh
pi /login openai-codex
```

Either the nodejs `pi-coding-agent` or the Rust pi works — the harness reads
both on-disk schemas. The login writes an `openai-codex` OAuth entry; the
harness refreshes the short-lived access token itself when it nears expiry.

## 2. Select the OAuth mode

Precedence is **CLI flag > config/env > default**.

### Via the worker CLI

```sh
harness-testing-worker --kind role --role engineer --user engineer \
  --agents real --auth chatgpt-oauth \
  --backend forgejo --base-url http://127.0.0.1:3000 --clock wall \
  --repo acme/service --root /tmp/unused
```

Overrides:

- `--codex-model <id>` — the Codex model id (default `gpt-5.3-codex`, the id A3
  live-validated a ChatGPT account serves; others returned `model is not
  supported when using Codex with a ChatGPT account`).
- `--auth-file <path>` — a non-default auth-file location.

### Via a config file / env (the launch-script bridge)

The launch script sources a config file (e.g. the example's
`config/harness.env`) that exports the env vars the worker reads:

```sh
HARNESS_AGENTS_AUTH=chatgpt-oauth        # deepseek | chatgpt-oauth
HARNESS_AGENTS_CODEX_MODEL=gpt-5.3-codex # optional override (default)
HARNESS_AGENTS_AUTH_FILE=/path/auth.json # optional override
```

A CLI flag overrides the matching env var; absent both, the test/dev default is
`chatgpt-oauth`. (The `harness-agents` **library** default stays `deepseek` so
production wiring is explicit.)

## 3. Setup errors surface before any work

If no login is found, the worker fails at startup with a clear setup error
naming `openai-codex` and pointing you back at `pi /login openai-codex` — it does
not silently start and stall on the first tick. (The DeepSeek mode fails the same
way when its key is missing.)

## 4. Verify

- **Gated live check (A3):** `HARNESS_CHATGPT_OAUTH=1 cargo test -p harness-agents
  --test chatgpt_oauth_live -- --ignored` runs one real decision against the
  Codex endpoint and a near-expiry refresh check.
- **End-to-end:** run the example from Track B (`examples/reference-delivery`) or
  the gated Forgejo multi-process test with `--agents real` and
  `HARNESS_AGENTS_AUTH=chatgpt-oauth`.

## Limits

- ChatGPT/Codex **subscription rate limits** apply.
- The OAuth access token is **short-lived**; the harness refreshes it
  automatically. A `401`/`403` means the login expired or lacks Codex access —
  re-run `pi /login openai-codex`.
- No token is ever logged or placed in an error.
