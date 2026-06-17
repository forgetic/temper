# LLM responders

Temper does not contain `pi_agent_rust`, concrete LLM provider SDKs, or
provider-auth wiring. Real LLM behavior lives outside this repository behind
process or worker protocols; Smith (`~/src/rust/smith`) is the reference
external implementation used by the dogfood/example deployments.

Temper keeps provider-neutral contracts and adapters. External implementations
should depend on the worker/daemon wire protocol for workflow role jobs and on
`temper-protocol-interaction` for interactive profile replies, not on Temper
runtime crates.

- Workflow role work is assigned as a concrete worker job through the
  [Worker/Daemon wire protocol](worker-daemon-wire-protocol.md). The assignment
  carries the role, repository, queue, artifact context, action, checkout
  capability, and allowed verdict vocabulary. The role agent completes that job
  and returns a structured result: a branch/diff for writable implementation, a
  declared verdict plus authored body/review/children when declared, or a
  structured failure/rejection when the job cannot be completed.
- Interactive profile replies use
  [Interactive process responder protocol](interactive-process-responder-protocol.md).
  The generic `temper-interaction` binary loads user-defined profile specs and
  deployment bindings; each binding selects a process responder command, args,
  cwd, env allow-list, and timeout for the declared responder id.
- `temper-interaction`, `temper-interaction-service`, `temper-runner`, and
  `temper-worker` validate request/reply shapes, proposal acceptance, process
  timeouts, exit status, assigned job results, declared verdicts, and redacted
  errors. They do not parse provider auth files or call model APIs.

Provider selection, OAuth/API-key handling, model ids, prompt implementation, and
live provider smoke tests are external-responder concerns documented outside
Temper. External responders may receive authority-neutral observability/context
fields for correlation, but they receive no Forge handle, Forge token, or
workflow mutation tools. All workflow and Forge mutations remain Temper-owned:
Temper validates branch outputs, verdict vocabulary, workflow transitions,
interaction proposals, and then applies the allowed mutations itself.

Smith-backed operator/dogfood examples live in the sibling Smith checkout under
`~/src/rust/smith/examples/`. Temper's checked-in `examples/reference-delivery/`
launcher intentionally uses deterministic fake agents with a real throwaway
Forgejo and real `forgejo-runner`.
