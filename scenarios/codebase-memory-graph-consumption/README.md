# Codebase-memory graph consumption

This checked-in feature scenario maps `ai/temper#953` to its plan
`ai/temper#954` on `agent/pr-for-feature-953`. It proves the narrow evidence
path for a graph result that is consumed by a later graph refinement, trace,
and current-root source reads before an exact repair.

## Contract

The real Forgejo, host Actions runner, standalone Temper, and deterministic
Jig agent run one retry-worker repair. After targeted stable-key discovery and
normalized ready confirmation of the active checkout root, the engineer makes
exactly five model-visible MCP calls in this order:

1. `search_graph` identifies the retry-worker evidence;
2. `search_code` refines `retry_worker_topic`;
3. `trace_path` follows that symbol to its caller;
4. `get_code_snippet` consumes the implementation from the confirmed root; and
5. a second `get_code_snippet` consumes the focused test from that root.

The generic live fixture rejects a different order, an extra or broad MCP call,
an unconfirmed provider identity, an unsuccessful call, or a source read not
served from the current root. It retains only safe tool names, aggregate call
counts, and normalized binding facts in run evidence; MCP arguments, source,
prompts, provider output, credentials, diagnostics, caches, and timing claims
are not checked in.

`codebase-memory-remediation` remains the complementary scenario for fresh-key
rebind confirmation and the typed unavailable-provider fallback. This scenario
does not weaken or replace that coverage.

```sh
cargo run -p temper-scenario-cli -- check scenarios/codebase-memory-graph-consumption
cargo dev-scenario-run scenarios/codebase-memory-graph-consumption
```
