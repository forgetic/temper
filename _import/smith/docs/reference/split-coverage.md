# Split coverage ledger

Three repositories share the delivery topology. Temper owns process protocols,
validation, runner authority, generic interaction services, transcripts,
proposal acceptance, deterministic fake tests, and production process wiring.
Smith owns orchestration: the worker/daemon loop, the git workspace plane, and
the worker ↔ agent process protocol. **Anvil** owns the concrete agent: the
pi-SDK-backed provider/auth/decision behavior, the coding-agent loop, the
product-manager and workflow-role responders, and all live provider gates.

## Ownership after the agent/orchestration split

| Area | Owner | Coverage |
| --- | --- | --- |
| Provider/auth/model calls | anvil | Provider/OAuth unit tests, manual live provider tests, and request-oracle checks (run from `../anvil`). |
| One-turn structured decisions | anvil (Temper validates replies) | Workflow-role decision tests plus provider live smokes in anvil; process reply validation in `temper-runner`. |
| Product-manager profile behavior | anvil (Temper owns conversations/manifests) | Prompt/mapping/response tests and `anvil-product-manager-responder` in anvil. |
| Worker/daemon loop + git plane | smith | `smith-worker` fake-daemon/workspace/coding-executor tests. |
| Worker ↔ agent process boundary | smith | Hermetic `coding_worker_e2e` drives spawn → step-progress → result against a deterministic protocol-speaking fake agent (CI). |
| Agent loop ⇄ jig e2e | anvil | Jig-backed coding-agent and sub-agent tests in anvil's CI. |
| Full-topology basic delivery | smith (operator) | `examples/basic-delivery/run.sh`, provider-free with `BASIC_DELIVERY_CODER=greeting`. |
| Live provider proofs | anvil | Manual-only OAuth and request-oracle gates. |

## Removed gates

Do not use these as coverage gates; the code they exercised moved or was
retired:

- `cargo test -p temper-agents ...` (Temper, pre-split)
- `temper-testing-worker --agents real`
- production `temper-worker --auth/--codex-model/--auth-file`
- smith's `coding_agent_e2e` / `basic_delivery_jig_e2e` under
  `smith-temper-agent-cli` (the crate moved to anvil; the topology e2e is the
  operator-driven example until re-pointed in CI)

## Active Smith gates

Run from this repository:

| Command | Protects | Where |
| --- | --- | --- |
| `cargo dev-test` | Default hermetic protocol, worker-loop, workspace, and e2e coverage. | CI |
| `cargo test -p smith-worker --test coding_worker_e2e` | The out-of-process agent boundary (spawn, step-progress relay, crash classes). | CI (part of dev-test) |
