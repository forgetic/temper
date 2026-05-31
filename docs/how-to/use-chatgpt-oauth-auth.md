# Use OAuth auth modes for the LLM agents

The real, in-process LLM agents (`harness-agents`) can authenticate with a
DeepSeek **API key** (pay-per-token), ChatGPT (OpenAI Codex) **OAuth** (a flat
subscription, no per-call cost), or opt-in Anthropic **OAuth**. This guide
covers the ChatGPT default and notes the Anthropic OAuth selection surface. See
ADR 0020 for the ChatGPT rationale.

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
HARNESS_AGENTS_AUTH=chatgpt-oauth        # deepseek | chatgpt-oauth | anthropic-oauth
HARNESS_AGENTS_CODEX_MODEL=gpt-5.3-codex # optional Codex override (default)
HARNESS_AGENTS_AUTH_FILE=/path/auth.json # optional shared auth-file override
```

A CLI flag overrides the matching env var; absent both, the test/dev default is
`chatgpt-oauth`. (The `harness-agents` **library** default stays `deepseek` so
production wiring is explicit.)

## 3. Anthropic OAuth opt-in

Anthropic OAuth is available but not the default. Log in once:

```sh
pi /login anthropic
```

Then select it explicitly:

```sh
HARNESS_AGENTS_AUTH=anthropic-oauth
HARNESS_AGENTS_ANTHROPIC_MODEL=claude-opus-4-8 # optional; this is the default
```

It reads the `anthropic` entry from the same shared auth file, tolerates both pi
schemas, refreshes near-expiry credentials, and injects Claude Code-compatible
HTTP identity headers per request. The worker has no `--anthropic-model` flag;
use the env var when overriding the default.

## 4. Setup errors surface before any work

If no login is found, the worker fails at startup with a clear setup error
naming `openai-codex` or `anthropic` and pointing you back at the matching
`pi /login ...` command — it does not silently start and stall on the first tick.
(The DeepSeek mode fails the same way when its key is missing.)

## 5. Verify

- **Gated live check (A3):** `HARNESS_CHATGPT_OAUTH=1 cargo test -p harness-agents
  --test chatgpt_oauth_live -- --ignored` runs one real decision against the
  Codex endpoint and a near-expiry refresh check.
- **Anthropic gated live check:** `HARNESS_ANTHROPIC_OAUTH=1 cargo test -p
  harness-agents --test anthropic_oauth_live -- --ignored` runs one real decision
  against the Anthropic endpoint.
- **End-to-end:** run the example from Track B (`examples/reference-delivery`) or
  the gated Forgejo multi-process test with `--agents real` and
  `HARNESS_AGENTS_AUTH=chatgpt-oauth` (or explicit `anthropic-oauth`).

## Limits

- ChatGPT/Codex or Anthropic **subscription rate limits** apply.
- OAuth access tokens are **short-lived**; the harness refreshes them
  automatically. A `401`/`403` means the login expired or lacks provider access —
  re-run the matching `pi /login ...`.
- No token is ever logged or placed in an error.
