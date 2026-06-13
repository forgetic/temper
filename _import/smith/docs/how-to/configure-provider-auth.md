# Configure provider auth

Provider auth belongs to the agent side: anvil's binaries (`anvil-agent` and
the responders) accept the same provider flags:

```sh
--auth deepseek|chatgpt-oauth|anthropic-oauth
--codex-model MODEL
--auth-file PATH
```

The binaries default to `chatgpt-oauth`. Run preflight (from the sibling
`../anvil` checkout) before wiring a provider into Temper:

```sh
cargo run --bin anvil -- preflight --auth chatgpt-oauth
```

## ChatGPT/OpenAI Codex OAuth

1. Log in once with pi:

   ```sh
   pi /login openai-codex
   ```

2. Anvil reads the `openai-codex` entry from `~/.pi/agent/auth.json`, accepting
   both nodejs and Rust pi schemas and preserving the schema on refresh.
3. Optional overrides:

   ```sh
   TEMPER_AGENTS_AUTH_FILE=/path/to/auth.json
   TEMPER_AGENTS_CODEX_MODEL=gpt-5.5
   ```

The Codex access token is short-lived. Anvil resolves and refreshes it per
request; live refresh may rotate the refresh token and write back to the auth
file.

## Anthropic OAuth

1. Log in once with pi:

   ```sh
   pi /login anthropic
   ```

   Select the Anthropic OAuth/subscription credential when prompted.

2. Optional overrides:

   ```sh
   TEMPER_AGENTS_AUTH_FILE=/path/to/auth.json
   TEMPER_AGENTS_ANTHROPIC_MODEL=claude-opus-4-8
   ```

Anvil injects the Claude Code-compatible request identity required by Anthropic
subscription OAuth.

## DeepSeek API key

Provide a key by env var or a gitignored local file:

```sh
export TEMPER_DEEPSEEK_API_KEY=...
# or
mkdir -p .cache
printf '%s' '...' > .cache/deepseek-api-key
```

`TEMPER_DEEPSEEK_API_KEY_PATH` overrides the file path.

## Coding-agent prompt overlays

`anvil-agent` also accepts `--config-dir PATH` to point at an operator
config dir holding optional prompt overlays (`prompts/engineer.md`,
`prompts/architect.md`, `prompts/reviewer.md`, and a shared
`prompts/coding-agent.md`). When unset it defaults to `$ANVIL_CONFIG_DIR`, then
`$XDG_CONFIG_HOME/anvil`, then `~/.config/anvil`. The checkout's root `AGENTS.md`
is injected as context by default. Missing dirs/files are a clean no-op. See
[Coding-agent prompt overlays and AGENTS.md](../reference/coding-agent-prompts.md)
for the full contract.

## Secrets discipline

- Never pass provider secrets on argv.
- Never commit auth files, copied `auth.json`, tokens, or `.env` files.
- When Temper launches an anvil responder, allow-list only provider env vars it
  must read; never allow-list Forge tokens or workflow credentials.
