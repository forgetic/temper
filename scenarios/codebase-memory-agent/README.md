# codebase-memory-agent

This scenario validates #82/#170: the engineer coding agent uses Temper's codebase-memory MCP integration through the generic live `manifest` runner.

The manifest runner boots the validation-grade stack (real Forgejo, host `forgejo-runner` Actions CI, real standalone Temper, and Jig fake LLM). The scenario configures a deterministic fake codebase-memory MCP server through Temper's normal agent tool configuration path. The fake server advertises safe tools plus hidden unsafe/index tools; Temper exposes only safe `codebase_memory_*` wrappers to the model, keeps `index_repository` internal, defaults the project to `actual-demo-project`, and returns `FAKE_MCP_SEARCH_RESULT` to the model before the engineer writes `MEMORY_NOTES.md`.

The scenario-owned `jig/codebase-memory-agent.json` script supplies the complete coding-agent tool loop; no global Jig fixture or scenario-name selector supplies its responses.

Run it through the sole manual live-run alias:

```sh
cargo run -p temper-scenario-cli -- check scenarios/codebase-memory-agent
cargo dev-scenario-run scenarios/codebase-memory-agent
```

The run uses the implicit manifest topology. Expected evidence includes
structured Temper events for tool configuration/exposure/hiding, MCP server
startup, `search_code` call/result, workspace diff production, and agent
completion.
