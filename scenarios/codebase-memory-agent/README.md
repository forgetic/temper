# codebase-memory-agent

This scenario validates #82: the engineer coding agent can use Temper's codebase-memory MCP integration end to end in a hermetic setup.

The hermetic runner starts a Jig fake LLM and a Python fake codebase-memory MCP server. The fake server advertises both safe tools and unsafe/indexing tools; Temper exposes only safe `codebase_memory_*` wrappers to the model, keeps `index_repository` internal, and injects the actual project discovered from `list_projects.root_path` when the model omits `project`.

Run it with:

```sh
cargo run -p temper-scenario-cli -- check scenarios/codebase-memory-agent
cargo run -p temper-scenario-cli -- run --tier hermetic scenarios/codebase-memory-agent
```

Expected evidence names the fake `search_code` MCP call, CODEBASE MEMORY prompt guidance, workspace project defaulting to `actual-demo-project`, and internal `index_repository` use with `repo_path`.
