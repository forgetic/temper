# Sequential graph evidence

This checked-in feature scenario maps `ai/temper#973` to plan
`ai/temper#974` on `agent/pr-for-feature-973`. It is distinct from the
historical `codebase-memory-graph-consumption` mapping for `#962`.

## Contract

The real Forgejo instance, host Actions runner, standalone Temper process, and
deterministic Jig engineer execute one delivery-worker repair. A provider-shaped
current-root fixture accepts a targeted decision only after its successful
predecessor, so dependent calls occur in separate model turns:

1. `search_graph` selects the delivery-worker refinement;
2. `search_code` selects the trace target;
3. `trace_path` selects the current-root implementation source;
4. `get_code_snippet` reads that implementation; and
5. its successful source result exposes the test-only target for the second
   `get_code_snippet` read.

Only after both current-root source reads does the Jig agent write the minimal
repair: every retry selects `canonical_topic` when available, without adding
comments or unrelated changes. The fixture rejects a different order, extra or
broad calls, a failed call, an unconfirmed binding, or a test read before its
implementation-source result; host CI validates the resulting repair.

## Privacy-safe evidence

Checked-in declarations and aggregate run evidence retain only safe tool names,
aggregate counts, normalized binding facts, and typed-correlation
completion/version/tool/kind facts. Provider arguments, targets, source,
prompts, results, digests, credentials, diagnostics, caches, and timing claims
are not promoted into the scenario corpus or aggregate reports. Ephemeral
fixture logs remain runtime review artifacts.

## Landing use

Run this mapped scenario against an exact feature head. After landing, build a
fresh detached exact-final-head candidate and require an enabled smoke meeting
the task, host, exact-patch, relevance, and current-root-source gates before
running the unchanged frozen enabled/disabled/unavailable matrix.

```sh
cargo dev-scenario-check
cargo dev-scenario-run scenarios/sequential-graph-evidence
```
