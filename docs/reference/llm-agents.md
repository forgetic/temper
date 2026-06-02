# LLM agents reference

`crates/harness-agents` is the production home for real, in-process LLM role
agents. It is the only crate that depends on `pi_agent_rust`; `harness-forge`,
`harness-workflow`, and `harness-runner` stay LLM-agnostic.

## Runtime boundary

- Agents receive work items from the runner and emit structured decisions.
- The SDK is run with no registered bash/file tools.
- Workflow state mutations happen only through authorized runner tools such as
  `RoleTools`.
- Implementation PR creation may invoke a declared-and-bound `coding_workspace`
  provider, but the LLM only sees the narrow tool metadata; the provider receives
  work-item context and returns a committed branch/head.
- Manifest-driven role agents are the production path; legacy reference-delivery
  adapters remain test/dev support for gated e2e scenarios.

## Auth modes

`ProviderConfig::from_auth` supports three modes:

- `deepseek` / API key: OpenAI-compatible DeepSeek route, with the key read from
  the configured env or cache path.
- `chatgpt-oauth`: OpenAI Codex OAuth from the shared pi auth file; this is the
  test/dev default because it uses a flat subscription.
- `anthropic-oauth`: Anthropic OAuth from the shared pi auth file, selected
  explicitly.

Credentials are read from env or the shared auth file, never from argv; errors
and debug output redact secrets. See `docs/how-to/use-chatgpt-oauth-auth.md` for
login, env, and verification commands.

## Rust and dependency notes

`harness-agents` pins its own `edition = "2024"` and `rust-version = "1.85"`
because of the SDK floor; the workspace root stays on the broader workspace
settings.

`pi_agent_rust 0.1.13` pulls `asupersync =0.3.1`, which needs the
API-compatible `franken-decision 0.3.1`. Keep the workspace `Cargo.lock` pin made
with:

```sh
cargo update -p franken-decision --precise 0.3.1
```

If `pi_agent_rust` is bumped, re-check the transitive constraints before
changing or dropping this pin. Lesson 0008 records the original failure mode.
