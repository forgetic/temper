# Working in delivery-policy

Use codebase-memory-mcp for structural code discovery: `search_graph` to find
symbols, `trace_path` for callers, and `get_code_snippet` for focused source.
Index this repository with `index_repository` first if it is not indexed.
Prefer graph discovery before file search. Shell search is appropriate for
documentation, configuration, string literals, or insufficient graph results.
If the graph provider is unavailable, record that fact and use shell tools.

Keep the crate dependency-free, modules cohesive, and `lib.rs` a thin facade.
Add focused behavioral tests, update README examples and CHANGELOG, and run
`cargo fmt --all -- --check` and `cargo test --offline --quiet` before completing
the work. Inspect the full diff. Do not weaken or remove existing tests.

In a Temper session, complete the work with `submit_for_pr` after these gates
pass. In a Codex session, finish with a brief result and the gates run; the
benchmark host collects the final workspace diff.
