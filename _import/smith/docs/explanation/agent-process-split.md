# The agent / orchestration split

> Restores the key decisions from the executed design doc
> `agent-process-split.md` (deleted after execution in `2a77fd8`); this is the
> durable home for the architecture it carried.

The coding agent lives in the sibling **`anvil`** repo and runs
**out-of-process** behind a small versioned protocol
(`crates/smith-agent-protocol`). Smith is **orchestration-only**:
`smith-worker` links no agent/LLM code, spawns the agent binary per job, and
relays the agent's step-progress checkpoints.

## Two planes, deliberately decoupled

1. **Orchestration plane (forge-based):** temper-daemon + smith-worker +
   Forgejo. Narrow, coarse, durable, human-cadence. This is all smith/temper
   know about. The forge does code lifecycle only — its narrowness is a
   feature.
2. **Control/observability plane (out-of-band, future):** rich/live — stream
   deltas, steering, abort, web UI. Owned by the agent side; smith/temper/forge
   know nothing about it. The agent-side API it will consume (the
   `AgentMachine` events + steering/abort handle) is preserved in anvil.

## Key decisions

- **Out-of-process agent**, decided primarily for *reusability* ("bring any
  agent that speaks the protocol"), with fault isolation as the bonus.
- **Smith owns the protocol; agents implement it.** The contract is the
  serde-only `smith-agent-protocol` crate: a `WorkspaceContext` file in
  (named by `$TEMPER_CODING_WORKSPACE_CONTEXT`), a `StepProgress` line stream
  on the agent's stdout, and a terminal `WorkspaceResult` file out (named by
  `$TEMPER_CODING_WORKSPACE_RESULT`). The reference implementation is
  `anvil-agent`; the examples' deterministic `greeting` stand-in speaks the
  same protocol unchanged.
- **Relay path: agent → worker → daemon → forge.** The agent has git
  credentials (push only) via the prepared workspace; it never calls the forge
  API. The worker has no forge-API client either (git push only) — the forge
  API is the **daemon's** job.
- **Step-progress = crash-recovery checkpoint channel**, not just
  observability: the agent pushes at coherent step boundaries and emits a
  marker; after a crash, a re-dispatched agent resumes from branch + marker.
  Resumability, not transactionality.
- **One correlation id** (`WorkspaceContext.correlation_key`) is the *only*
  bridge between the planes.
- **Bright-line rule:** plane-1 progress carries only durable, human-facing PR
  state (checklist line, phase, pushed sha). Anything high-frequency (token
  deltas, tool calls) belongs to plane 2. If a field is added "so the UI can
  show X", it is on the wrong plane.
- Exit mapping in the worker: non-zero agent exit ⇒ transient
  (re-dispatchable); missing/invalid result file ⇒ permanent.

## Pointers

- The protocol: `crates/smith-agent-protocol/src/lib.rs` (doc comments carry
  the wire-level detail).
- The worker side: `crates/smith-worker/src/out_of_process_runner.rs` (spawn +
  stdout relay) and the `ProgressSink` seam on `AgentRunner`.
- The agent side: the `anvil` sibling repo (`src/bin/anvil-agent.rs`).
- Hermetic e2e: `crates/smith-worker/tests/coding_worker_e2e.rs` drives the
  full register→assign→spawn→progress→result path against a deterministic
  protocol-speaking fake agent.
