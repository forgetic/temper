# Testing and coverage

## Default hermetic Smith validation

Run the same provider-free validation shape that CI uses for ordinary pull
requests:

```sh
cargo fmt --all
cargo dev-clippy
cargo dev-check
cargo dev-test
```

`cargo dev-test` is the default hermetic test gate. It compiles ignored tests
but does not run ignored e2e checks.

Useful focused areas:

- the worker ↔ agent process protocol: `cargo test -p smith-agent-protocol`;
- the worker loop, git workspace plane, and coding executor:
  `cargo test -p smith-worker --test fake_daemon --test workspace --test
  coding_executor`;
- the out-of-process agent boundary end to end (hermetic):
  `cargo test -p smith-worker --test coding_worker_e2e`. This drives the full
  register → poll → assign → spawn → step-progress → commit/push → result path
  against a deterministic protocol-speaking `smith-fake-agent` binary — no LLM,
  no cross-repo binary path.

## Agent-side coverage lives in anvil

Provider/auth parsing, the coding-agent loop, responders, and the jig-backed
agent e2e tests moved to the sibling `anvil` repository with the agent. Run
them from `../anvil` (see anvil's README); live OAuth/provider gates are
manual-only there as they were here.

## Topology smoke (operator-driven)

For a full daemon/worker/Forgejo run, use `examples/basic-delivery/run.sh`.
`BASIC_DELIVERY_CODER=greeting` gives a provider-free smoke of the whole
orchestration path; the default `anvil` coder spawns the real `anvil-agent`
built from the sibling checkout.

## Temper-side coverage that protects Smith integration

Run from `../temper` when changing a protocol boundary:

```sh
cargo test -p temper-worker-protocol
```

Temper's tests should remain provider-free. Real provider behavior belongs in
anvil's manual live/provider gates.
