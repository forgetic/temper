# anvil

The **agent side** of the agent / orchestration split. `anvil` owns the coding
agent — the pi-SDK-backed LLM loop, providers/auth, the sans-IO `AgentMachine`,
sub-agents, and the responders — as an independent project that knows nothing
about Forgejo/GitHub orchestration.

It is the counterpart to **smith** (orchestration: `smith-worker`) and
**temper** (the daemon). `smith-worker` runs an `anvil` agent **out-of-process**,
spawning its binary and speaking smith's small **versioned process protocol**
(`smith-agent-protocol`: context in → step-progress + result out). Smith owns
that contract; anvil implements it. The agent has git credentials (via the
prepared workspace) to push commits, and nothing else; the worker side owns all
forge-API interaction. See `smith/docs/explanation/agent-process-split.md` for
the full design.

Anything real-time — live stream deltas, steering, abort, a web UI — belongs to
a separate **control/observability plane** that consumes `anvil`'s agent-side
API (the `AgentMachine` events + steering/abort handle). `smith`, `temper`, and
the forge know nothing about that plane; one correlation id is the only bridge.

## Layout

Library crates live under `crates/`; the project binaries are minimal glue in
`src/bin/` (same pattern as temper):

- `src/bin/anvil-agent.rs` — **`anvil-agent`**, the out-of-process coding agent
  the worker spawns (the process-protocol boundary).
- `src/bin/anvil.rs` — **`anvil`**, the project CLI (`preflight`, `version`).
- `src/bin/anvil-product-manager-responder.rs`,
  `src/bin/anvil-workflow-role-decision.rs` — stdin/stdout responder binaries
  for temper's interaction protocol.

## Crates

- `anvil-io-engine` — anvil's copy of the sans-IO completion-engine driver
  (`Machine` core + `Executor` shell + `drive` loop). Deliberately duplicated
  from smith/temper so anvil has an independent dependency surface.
- `anvil-agent` — the sans-IO LLM agent loop + sub-agents (`AgentMachine`,
  shell, `SubAgentTool`).
- `anvil-temper-agent` — provider/auth/decision/coding-loop core and responders.

## Build

Sibling layout matters: `anvil`, `smith`, `temper`, and `jig` are checked out
side by side, so the `../{jig,smith,temper}/...` path-deps resolve unchanged.

Build with `-j1` on the constrained host (no swap) to avoid OOM during pi-SDK
compiles.
