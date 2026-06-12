# anvil

The **agent side** of the agent / orchestration split. `anvil` owns the coding
agent — the pi-SDK-backed LLM loop, providers/auth, the sans-IO `AgentMachine`,
sub-agents, and the responders — as an independent project that knows nothing
about Forgejo/GitHub orchestration.

It is the counterpart to **smith** (orchestration: `smith-worker`) and
**temper** (the daemon). `smith-worker` runs an `anvil` agent **out-of-process**,
spawning its binary and speaking a small **versioned process protocol**
(context in → step-progress + result out). The agent has git credentials (via
the prepared workspace) to push commits, and nothing else; the worker owns all
forge-API interaction. See `smith/agent-process-split.md` for the full design.

Anything real-time — live stream deltas, steering, abort, a web UI — belongs to
a separate **control/observability plane** that consumes `anvil`'s agent-side
API (the `AgentMachine` events + steering/abort handle). `smith`, `temper`, and
the forge know nothing about that plane; one correlation id is the only bridge.

## Crates

- `smith-io-engine` — anvil's copy of the sans-IO completion-engine driver
  (`Machine` core + `Executor` shell + `drive` loop). Deliberately duplicated
  from smith/temper so anvil has an independent dependency surface.
- `smith-agent` — the sans-IO LLM agent loop + sub-agents (`AgentMachine`,
  shell, `SubAgentTool`).
- `smith-temper-agent` — provider/auth/decision/coding-loop core and responders.
- `smith-temper-agent-cli` — small utility/responder binaries.

> Crate names still carry the `smith-`/`temper-` prefixes from their origin in
> the smith workspace. Renaming to shed those prefixes is deliberate later work
> (mechanical port first); the worker no longer links these crates, so the names
> are cosmetic.

## Build

Sibling layout matters: `anvil`, `smith`, `temper`, and `jig` are checked out
side by side, so the `../../../{jig,temper}/...` path-deps resolve unchanged.

Build with `-j1` on the constrained host (no swap) to avoid OOM during pi-SDK
compiles.
