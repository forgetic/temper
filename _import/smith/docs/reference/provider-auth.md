# Provider auth

Smith supports three auth modes for one-turn LLM decisions.

| Auth choice | Provider route | Credential source |
| --- | --- | --- |
| `chatgpt-oauth` | OpenAI Codex Responses (`openai-codex`) | `openai-codex` OAuth entry in `~/.pi/agent/auth.json` |
| `anthropic-oauth` | Anthropic Messages (`anthropic`) | `anthropic` OAuth entry in `~/.pi/agent/auth.json` |
| `deepseek` | OpenAI-compatible chat completions | `TEMPER_DEEPSEEK_API_KEY` or `.cache/deepseek-api-key` |

The library default is `deepseek`; Smith CLI/responder binaries default to
`chatgpt-oauth` for local/dev cost control.

## Public flags

All responder binaries accept:

```text
--auth deepseek|chatgpt-oauth|anthropic-oauth
--codex-model MODEL
--auth-file PATH
```

`--codex-model` applies only to ChatGPT/OpenAI Codex. Anthropic model selection
uses `TEMPER_AGENTS_ANTHROPIC_MODEL`.

## Public env vars

| Env var | Meaning |
| --- | --- |
| `TEMPER_AGENTS_AUTH_FILE` | Override shared pi auth file for OAuth modes. |
| `TEMPER_AGENTS_CODEX_MODEL` | Override Codex model id; default `gpt-5.5`. |
| `TEMPER_AGENTS_ANTHROPIC_MODEL` | Override Anthropic model id; default `claude-opus-4-8`. |
| `TEMPER_DEEPSEEK_API_KEY` | DeepSeek key, highest precedence. |
| `TEMPER_DEEPSEEK_API_KEY_PATH` | DeepSeek key file path; default `.cache/deepseek-api-key`. |
| `TEMPER_AGENTS_AUTH` | Ignored by normal responders; used by Smith's Forgejo e2e to choose auth. |

## OAuth auth-file compatibility

Smith reads both pi auth-file schemas:

- nodejs pi: `type:"oauth"`, `access`, `refresh`, `expires`;
- Rust pi SDK: `type:"o_auth"`, `access_token`, `refresh_token`, `expires`.

Refresh writes back in the same schema that was read and preserves unknown
fields. Errors must never include access or refresh token bytes.

## Provider-specific request rules

- ChatGPT/OpenAI Codex resolves the access-token bearer fresh per decision and
  requests low reasoning effort.
- Anthropic OAuth resolves the bearer fresh per decision, injects Claude
  Code-compatible identity headers, and sends the required Claude Code system
  identity as the first system block.
- DeepSeek pins temperature to `0.0` for deterministic decisions.
