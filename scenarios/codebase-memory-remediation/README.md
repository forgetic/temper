# Codebase-memory remediation

This is the feature-head scenario for `ai/temper#935`, planned by
`ai/temper#936`, on `agent/pr-for-feature-935`. It exercises the production
codebase-memory path through the generic live manifest runner: real Forgejo,
the host Actions runner, standalone Temper, and the worker-to-agent tool
configuration path. Only the MCP provider and LLM are deterministic fixtures.

## Claim → stimulus → observable → assertion

- **Claim:** A fresh stable project cannot stand in for a current checkout:
  every prepared checkout is explicitly rebound under its one stable provider
  key before a model-visible source read, while a systemic provider failure
  still falls back safely instead of causing a retry storm.
- **Stimulus:** The deterministic provider begins with one fresh stable key
  bound to a retired prepared checkout. It delays the explicit current-root
  rebind for 750 ms, serves `src/lib.rs` only from the resulting active binding,
  returns bounded graph guidance for the retry-worker repair, then returns one
  fixture-only systemic failure.
- **Observable:** The run retains fresh-rebind lifecycle events, the targeted
  request inventory, one stable alias-to-provider translation, readiness delay,
  safe current-root source marker, safe failure category, bounded result marker,
  conventional fallback sequence, repaired Rust file, host checks, and CI
  convergence. The generic fixture rejects global inventory, path-keyed
  projects, an unconfirmed rebind, or source served from the retired binding.
- **Assertion:** The graph result selects the implementation and test; the
  explicit rebind completes within the configured deadline; the model receives
  safe repository-relative `src/lib.rs` source from the current binding; and,
  after the typed failure, `grep`, `find`, and `read` proceed without another
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
provider stderr, cache databases, run identifiers, temporary paths, and
generated runtime evidence do not belong in this bundle. Deployment remains
held unless every frozen gate passes.

```sh
cargo run -p temper-scenario-cli -- check scenarios/codebase-memory-remediation
cargo dev-scenario-run scenarios/codebase-memory-remediation
```
