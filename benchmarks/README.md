# Agent-session benchmark corpus

`benchmarks/` contains repeatable inputs for measuring and describing one coding
agent session. It is separate from the [`scenarios/`](../scenarios/README.md)
validation corpus: benchmarks report agent-loop structure and performance,
while scenarios prove workflow correctness and convergence.

The operator command is `temper-benchmark`. See
[Benchmark agent sessions](../docs/how-to/benchmark-agent-sessions.md) for trace
analysis, harness and live runs, repetitions, retention, and comparisons. The
[codebase map](../docs/explanation/codebase-map.md) identifies the
implementation crates and process boundary.

## Checked-in benchmark

`agent-sessions/cross-cutting-rust-change/` is the deterministic reference
benchmark. It contains:

- a small Rust crate copied into a fresh temporary workspace per repetition;
- a valid `WorkspaceContext` for one writable repository;
- a local `.temper/pre-push.toml` gate and a host post-run test command;
- a Jig script that drives the real `temper-agent` process through multiple
  reads and writes, validation, a later mutation, revalidation, and terminal
  `submit_for_pr`.

Run the complete CI-safe lane from the repository root:

```sh
cargo dev-benchmark-harness
```

The `temper-dev` driver builds `temper-agent`, runs the checked-in manifest, and
verifies both run and aggregate summaries. Artifacts are written below
`target/benchmark-harness/cross-cutting-rust-change/`. Every harness report says
that it is plumbing and structural evidence, not representative LLM
performance.

Do not add credentials, generated artifacts, live timing baselines, databases,
or dashboard state to this directory. Fixture changes should remain small,
reviewable, deterministic, and provider-independent.
