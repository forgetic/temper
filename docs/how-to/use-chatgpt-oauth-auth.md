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

## Product-manager chat

```sh
cd ~/src/rust/smith
cargo build -p smith-temper-agent-cli --bin smith-product-manager-responder

cd ~/src/rust/temper
TEMPER_PRODUCT_CHAT_RESPONDER_COMMAND=../smith/target/debug/smith-product-manager-responder \
TEMPER_PRODUCT_CHAT_RESPONDER_ARGS_JSON='["--auth","chatgpt-oauth"]' \
  temper-product-manager-chat repl --base-url https://git.ekanayaka.io --repo ai/temper
```

External clients should still call Temper's product-chat service/API. Smith's
process is the responder implementation, not the public chat frontend.
