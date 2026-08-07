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

## Checked-in benchmarks

`agent-sessions/cross-cutting-rust-change/` is the deterministic structural
reference benchmark. It contains:

- a small Rust crate copied into a fresh temporary workspace per repetition;
- a valid `WorkspaceContext` for one writable repository;
- a local `.temper/pre-push.toml` gate and a host post-run test command;
- a Jig script that drives the real `temper-agent` process through multiple
  reads and writes, validation, a later mutation, revalidation, and terminal
  `submit_for_pr`.

Model-turn batching means that one model response emits multiple cohesive tool
calls. The fixture's initial implementation, export, and integration-test
writes share one model response; the later documentation write is a second
mutation turn. This batching does not make tool execution concurrent.
Independent reads may run concurrently, but `write`, `edit`, process, network,
and other barrier calls remain serialized.

`agent-sessions/codebase-memory-routing-repair/` adds a controlled three-condition
benchmark under one `codebase-memory-routing-repair` identity. The same task,
fixture, Jig model identity, host validation, exact expected patch, and cache
annotation run with codebase-memory enabled, disabled, or forced systemically
unavailable. Its README predeclares the live material-improvement criterion and
explains why deterministic Jig timing is not effectiveness evidence.

Run the complete CI-safe lane from the repository root:

```sh
cargo dev-benchmark-harness
```

The `temper-dev` driver builds `temper-agent`, runs the structural manifest and
all three controlled codebase-memory conditions, and verifies run and aggregate
summaries. The structural checks include mutation turns, single-mutation turns,
and maximum mutations per turn. The controlled checks include complete
compound-shell classification, targeted graph-result consumption, deterministic
cold-then-warm stable readiness, a bounded privacy-safe unavailable fallback,
host validation, and exact diff correctness. Those fixture signals verify
harness behavior, not deployment eligibility or live model effectiveness.
Malformed or missing scope ancestry is an ingestion error,
not unavailable historical evidence. If an ingested trace contains a successful mutation without usable model-turn
identity, all three mutation-turn metrics are unavailable and the summary emits
`StructureEvidenceUnavailable` rather than reporting zero. Artifacts are
written below `target/benchmark-harness/`, grouped by benchmark identity and
condition.

Every deterministic harness report is plumbing and structure evidence only, not
representative LLM performance. Use repeated live runs to draw behavioral or
performance conclusions. Jig timing must not become a CI timing gate.

Do not add credentials, generated artifacts, live timing baselines, databases,
or dashboard state to this directory. Fixture changes should remain small,
reviewable, deterministic, and provider-independent.
