# codebase-memory-agent

This scenario validates #82/#170 and the stable lifecycle feature #860: the engineer coding agent uses Temper's codebase-memory MCP integration through the generic live `manifest` runner.

The manifest runner boots the validation-grade stack (real Forgejo, host `forgejo-runner` Actions CI, real standalone Temper, and Jig fake LLM). Its persistent deterministic provider starts with 320 historical Temper workspaces plus stable, active, unrelated, and ambiguous protected records, a fixed cache-byte estimate, and a four-second global-list delay. Temper must use targeted `index_status`, derive a checkout-independent `temper-v1-*` identity, avoid the delayed inventory path during the normal session, and make only safe tools model-visible. The final `search_code` result must use the same stable project that the internal single upsert selected before the engineer writes `MEMORY_NOTES.md`.

The scenario-owned `jig/codebase-memory-agent.json` script supplies the complete coding-agent tool loop; no global Jig fixture or scenario-name selector supplies its responses.

Run it through the sole manual live-run alias:

```sh
cargo run -p temper-scenario-cli -- check scenarios/codebase-memory-agent
cargo dev-scenario-run scenarios/codebase-memory-agent
```

The run uses the implicit manifest topology. Expected evidence includes
structured Temper events for tool configuration/exposure/hiding, MCP server
startup, `search_code` call/result, workspace diff production, and agent
completion. The generated report includes the provider call log, project identity,
request counts, cache fixture, and preservation categories. Lifecycle cleanup is
dry-run-first and bounded; active, stable, unrelated, and ambiguous records are
never eligible for deletion, and the injected `legacy-temper-000` failure remains
isolated for an idempotent follow-up pass.
