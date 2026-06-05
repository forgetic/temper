# LLM responders

Temper does not contain `pi_agent_rust`, `temper-agents`, concrete LLM provider
SDKs, or provider-auth wiring. Real LLM behavior lives outside this repository
behind process protocols; Smith (`~/src/rust/smith`) is the reference external
implementation used by the dogfood/example deployments.

Temper keeps only provider-neutral contracts and adapters. External responders
should depend on `temper-process-protocol` or implement equivalent JSON DTOs,
not on Temper runtime crates.

- Workflow role decisions use
  [Workflow-role decision process protocol](workflow-role-decision-process-protocol.md).
  Production `temper-worker --kind role` requires `--role-decision-command` (or
  `TEMPER_WORKER_ROLE_DECISION_COMMAND`) and passes only allow-listed env vars to
  the child process.
- Interactive profile replies use
  [Interactive process responder protocol](interactive-process-responder-protocol.md).
  The generic `temper-interaction` binary loads user-defined profile specs and
  deployment bindings; each binding selects a process responder command, args,
  cwd, env allow-list, and timeout for the declared responder id.
- `temper-interaction`, `temper-interaction-service`, `temper-runner`, and
  `temper-worker` validate request/reply shapes, authorized actions, proposal
  acceptance, process timeouts, exit status, and redacted errors. They do not
  parse provider auth files or call model APIs.

Provider selection, OAuth/API-key handling, model ids, prompt implementation, and
live provider smoke tests are external-responder concerns documented outside
Temper. External responders may receive authority-neutral observability/context
fields for correlation, but they receive no Forge handle, Forge token, or
workflow mutation tools. All mutations remain Temper `RoleTools` or explicit
interaction acceptance executor actions after reply validation.

Smith-backed operator/dogfood examples live in the sibling Smith checkout under
`~/src/rust/smith/examples/`. Temper's checked-in `examples/reference-delivery/`
launcher intentionally uses deterministic fake agents with a real throwaway
Forgejo and real `forgejo-runner`.
