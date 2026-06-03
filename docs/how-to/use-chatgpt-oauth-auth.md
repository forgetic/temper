# Configure LLM responder auth through Smith

Temper does not read provider auth files, hold API keys for model providers, or
link `pi_agent_rust`. For real LLM behavior, run a process responder such as
Smith and pass its provider/auth arguments through Temper's responder config.

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

## Product-manager chat

```sh
cd ~/src/rust/smith
cargo build -p smith-temper-agent-cli --bin smith-product-manager-responder

cd ~/src/rust/temper
TEMPER_PRODUCT_CHAT_RESPONDER_COMMAND=../smith/target/debug/smith-product-manager-responder \
TEMPER_PRODUCT_CHAT_RESPONDER_ARGS_JSON='["--auth","chatgpt-oauth"]' \
  temper-product-manager-chat repl --base-url https://git.ekanayaka.io --repo ai/temper
```

Smith owns the accepted `--auth`, model, and auth-file options plus provider
preflight. Temper treats these arguments as opaque child-process configuration.
