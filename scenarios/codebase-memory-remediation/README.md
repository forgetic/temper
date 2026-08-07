# Codebase-memory remediation

This is the feature-head scenario for `ai/temper#944`, planned by
`ai/temper#945`, on `agent/pr-for-feature-944`. It exercises the production
codebase-memory path through the generic live manifest runner: real Forgejo,
the host Actions runner, standalone Temper, and the worker-to-agent tool
configuration path. Only the MCP provider and LLM are deterministic fixtures.

## Claim → stimulus → observable → assertion

- **Claim:** A fresh requested stable key cannot stand in for a current
  checkout. The provider may normalize that key, so the normalized provider
  identity is retained only after a targeted ready `index_status` confirms the
  exact canonical `root_path`; graph and source reads then use that confirmed
  identity. A systemic provider failure still falls back safely instead of
  causing a retry storm.
- **Stimulus:** The deterministic provider begins with one fresh requested
  stable key bound to a retired prepared checkout. It delays the explicit
  current-root upsert for 750 ms, returns a normalized provider identity from
  that upsert, then confirms the normalized identity with a targeted ready
  `index_status` containing the exact canonical `root_path`. It serves
  `src/lib.rs` only from the resulting active binding, returns bounded graph
  guidance for the retry-worker repair, then returns one fixture-only systemic
  failure.
- **Observable:** The run retains the two-call confirmation inventory,
  requested and confirmed identities, normalized graph and source request
  identities, one retained current-root binding, readiness delay, safe source
  marker, safe failure category, bounded result marker, conventional fallback
  sequence, repaired Rust file, host checks, and CI convergence. The generic
  fixture rejects global inventory, path-keyed projects, an unconfirmed rebind,
  or reads served from the retired binding.
- **Assertion:** Initial discovery uses the requested stable key; the upsert
  identity must be normalized and is usable only after targeted ready
  confirmation of the exact canonical `root_path`; the graph result selects the
  implementation and test; the model receives safe repository-relative
  `src/lib.rs` through that confirmed identity; and, after the typed failure,
  `grep`, `find`, and `read` proceed without another MCP call or raw provider
  text reaching the model.
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
