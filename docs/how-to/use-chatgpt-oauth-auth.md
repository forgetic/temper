# Configure Smith LLM responders from Temper

Temper does not read provider auth files, hold model-provider API keys, or link
`pi_agent_rust`. For provider login, model ids, auth-file overrides, and live
provider preflight, read Smith's docs in `~/src/rust/smith/docs/`.

Temper only launches responder processes and treats their args/env as opaque
child-process configuration.

## Workflow role workers

Build Smith's role responder and point `temper-worker` at it:

```sh
cd ~/src/rust/smith
cargo build -p smith-temper-agent-cli --bin smith-workflow-role-decision

cd ~/src/rust/temper
TEMPER_WORKER_ROLE_DECISION_COMMAND=../smith/target/debug/smith-workflow-role-decision \
TEMPER_WORKER_ROLE_DECISION_ARGS_JSON='["--auth","chatgpt-oauth"]' \
  temper-worker --kind role ...
```

Use `TEMPER_WORKER_ROLE_DECISION_ENV_ALLOWLIST` only for provider variables that
Smith explicitly documents. Do not allow-list Forge tokens.

## Interactive profiles

Interactive LLM responders are selected through the generic interaction
deployment binding file, not through profile-specific Temper binaries or env
names. Dogfood's example product-manager profile generates that binding for you:

```sh
cd ~/src/rust/smith
cargo build -p smith-temper-agent-cli --bin smith-product-manager-responder

cd ~/src/rust/smith/examples/dogfood
./run.sh product-chat
```

For a custom profile, set the binding file's `responders.<id>.command`, `args`,
`cwd`, `env_allowlist`, and `timeout_secs`, then launch `temper-interaction repl`
or `temper-interaction serve` with `--spec` and `--bindings`. External clients
should call Temper's generic interaction service/API; Smith's process is only the
responder implementation.
