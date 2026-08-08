# Codebase-memory graph consumption

This checked-in feature scenario maps `ai/temper#962` to its plan
`ai/temper#963` on `agent/pr-for-feature-962`. It proves the narrow typed
evidence path for a graph result that is consumed by a later graph refinement,
trace, and current-root source reads before an exact repair.

## Contract

The real Forgejo, host Actions runner, standalone Temper, and deterministic
Jig agent run one retry-worker repair. After targeted stable-key discovery and
normalized ready confirmation of the active checkout root, the engineer makes
exactly five provider-shaped model-visible MCP calls in this order:

1. `search_graph` identifies the retry-worker evidence;
2. `search_code` refines `retry_worker_topic`;
3. `trace_path` follows that symbol to its caller;
4. `get_code_snippet` consumes the implementation from the confirmed root; and
5. a second `get_code_snippet` consumes the focused test from that root.

The trusted wrapper emits a complete V1 typed correlation only for a successful
closed target extracted from one of those calls. The scenario asserts the
ordered tool and target-kind sequence plus exactly five complete correlations;
it does not treat a generic successful RPC as relevant. The generic live
fixture rejects a different order, an extra or broad MCP call, an unconfirmed
provider identity, an unsuccessful call, or a source read not served from the
current root.

Checked-in run evidence retains only safe tool names, aggregate call counts,
normalized binding facts, and typed-correlation completion/version/tool/kind
facts. MCP arguments, correlation digests, source, prompts, provider output,
credentials, diagnostics, caches, and timing claims are not checked in. The
fixture's private argument log and diagnostic trace are runtime review
artifacts, never scenario-corpus evidence.

## Live evidence ordering

This scenario validates the live workflow contract; it is not a live
effectiveness approval. After the feature branch has an exact final head, run a
fresh enabled smoke with complete 5/5 typed relevance and at least 50%
relevance before starting the unchanged enabled/disabled/unavailable 5x3
matrix. Do not count the failed `c388453` smoke or weaken either threshold.
Retain only the approved privacy-safe aggregate report from a later successful
candidate; keep runtime traces and fixture logs out of the repository.

`codebase-memory-remediation` remains the complementary scenario for fresh-key
rebind confirmation and the typed unavailable-provider fallback. This scenario
does not weaken or replace that coverage.

```sh
cargo run -p temper-scenario-cli -- check scenarios/codebase-memory-graph-consumption
cargo dev-benchmark-harness
cargo dev-scenario-run scenarios/codebase-memory-graph-consumption
```
