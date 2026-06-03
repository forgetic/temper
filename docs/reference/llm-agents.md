# LLM responders

Temper no longer contains `pi_agent_rust`, `temper-agents`, or concrete LLM
provider/auth wiring. Real LLM behavior lives outside this repository behind
process protocols. Smith (`~/src/rust/smith`) is the reference implementation for
pi-SDK-backed workflow-role decisions and the product-manager interactive
profile.

Temper keeps only provider-neutral contracts and adapters:

- Workflow role decisions use
  [Workflow-role decision process protocol](workflow-role-decision-process-protocol.md).
  Production `temper-worker --kind role` requires `--role-decision-command` (or
  `TEMPER_WORKER_ROLE_DECISION_COMMAND`) and passes only allow-listed env vars to
  the child process.
- Product-manager chat uses
  [Interactive process responder protocol](interactive-process-responder-protocol.md).
  `temper-product-manager-chat` requires `--responder-command` (or
  `TEMPER_PRODUCT_CHAT_RESPONDER_COMMAND`).
- `temper-interaction`, `temper-runner`, and `temper-production` validate
  request/reply shapes, authorized actions, proposal filing, process timeouts,
  exit status, and redacted errors. They do not parse provider auth files or call
  model APIs.

Provider selection, OAuth/API-key handling, model ids, prompt implementation, and
live provider smoke tests are Smith-owned concerns. Pass Smith arguments through
Temper's `*_ARGS_JSON` / repeated CLI arg flags and use the corresponding env
allow-list only for names that the responder must read.
