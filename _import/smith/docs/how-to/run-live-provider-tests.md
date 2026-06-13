# Run live provider checks

The provider code and these tests live in the sibling **anvil** checkout — run
every command on this page from `../anvil`. Do not use them in CI: they can
call real model providers, refresh real OAuth credentials, or require real API
keys.

For the CI-safe hermetic gates, see
[Testing and coverage](../reference/testing.md).

## ChatGPT/OpenAI Codex OAuth

Run `pi /login openai-codex` first, then:

```sh
TEMPER_CHATGPT_OAUTH=1 \
  cargo test --test chatgpt_oauth_live -- --ignored --nocapture
```

The refresh check may rotate the refresh token and write the refreshed credential
back to the real auth file.

The request-oracle leg for ChatGPT/OpenAI Codex OAuth is also manual-only:

```sh
TEMPER_CHATGPT_OAUTH=1 \
  cargo test -p anvil-temper-agent \
  --test jig_request_oracle \
  --features test-provider-base-url-override \
  -- --ignored --nocapture
```

## Anthropic OAuth

Run `pi /login anthropic` first, then:

```sh
TEMPER_ANTHROPIC_OAUTH=1 \
  cargo test --test anthropic_oauth_live -- --ignored --nocapture
```

The request-oracle leg for Anthropic OAuth is also manual-only:

```sh
TEMPER_ANTHROPIC_OAUTH=1 \
  cargo test -p anvil-temper-agent \
  --test jig_request_oracle \
  --features test-provider-base-url-override \
  -- --ignored --nocapture
```

## DeepSeek/OpenAI-compatible request oracle

This manual request-oracle leg requires a real DeepSeek/OpenAI-compatible API key
and makes real provider calls. Provide the key inline or via
`TEMPER_DEEPSEEK_API_KEY_PATH`:

```sh
TEMPER_DEEPSEEK_REQUEST_ORACLE=1 \
TEMPER_DEEPSEEK_API_KEY=... \
  cargo test -p anvil-temper-agent \
  --test jig_request_oracle \
  --features test-provider-base-url-override \
  -- --ignored --nocapture
```

The same test target also contains the OAuth request-oracle legs above; each leg
runs only when its own provider gate is set.

## Real Forgejo + real agent checks

The full-topology path with a real agent is the operator-driven
`examples/basic-delivery/run.sh` (default coder: the sibling checkout's
`anvil-agent` with `--auth chatgpt-oauth`). There is no real-provider CI gate
on this checkout; if a future test uses real provider-backed agents with
Forgejo, keep it ignored and document its live provider gate here rather than
in CI.
