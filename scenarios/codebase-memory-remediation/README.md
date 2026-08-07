# Codebase-memory remediation

This is the feature-head scenario for `ai/temper#908`, planned by
`ai/temper#909`, on `agent/pr-for-feature-908`. It exercises the production
codebase-memory path through the generic live manifest runner: real Forgejo,
the host Actions runner, standalone Temper, and the worker-to-agent tool
configuration path. Only the MCP provider and LLM are deterministic fixtures.

## Claim → stimulus → observable → assertion

- **Claim:** A concrete defect receives targeted graph guidance, a cold stable
  project becomes usable before the first graph call, and a systemic provider
  failure falls back safely instead of causing a retry storm.
- **Stimulus:** The fixture reports the prepared repository as missing, delays
  its one internal stable upsert for 750 ms, returns a bounded graph result for
  the retry-worker implementation and focused test, then returns one
  fixture-only systemic failure.
- **Observable:** The run retains the graph-call inventory, stable
  alias-to-provider translation, readiness delay, safe failure category,
  bounded result marker, conventional fallback sequence, repaired Rust file,
  host checks, and CI convergence.
- **Assertion:** The first targeted graph result selects the implementation and
  test, the delayed cold project is ready inside the configured deadline, and
  after the typed failure `grep`, `find`, and `read` proceed without another
  MCP call or raw provider text reaching the model.
- **Runtime budget:** 600 seconds.

The fixture's exact repair remains deliberately small: `src/lib.rs` must use
the canonical topic for every retry while the focused Rust test preserves the
unaliased case. This makes guidance consumption and host validation observable
without treating the deterministic fixture as a model-effectiveness result.

## Complementary controlled benchmark

The scenario owns workflow convergence and the bounded live-stack behavior. The
checked-in
[`codebase-memory-routing-repair`](../../benchmarks/agent-sessions/codebase-memory-routing-repair/README.md)
benchmark owns compound-shell classification coverage, byte-exact patch
validation, decision-relevance accounting, and the enabled/disabled/forced-
unavailable comparison protocol. Its enabled harness run records a cold
targeted search followed by a warm stable-project search; the focused agent
suite independently covers the same stable identity across relocated checkouts.

The benchmark's five matched production-provider repetitions per condition are
an operator-run deployment gate, not a scenario timing claim. Retain only a
privacy-reviewed aggregate report; credentials, transcripts, tool payloads,
provider stderr, cache databases, run identifiers, and temporary paths do not
belong in this bundle. Deployment remains held unless every frozen gate passes.

```sh
cargo run -p temper-scenario-cli -- check scenarios/codebase-memory-remediation
cargo dev-scenario-run scenarios/codebase-memory-remediation
```
