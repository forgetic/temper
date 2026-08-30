use super::*;

#[test]
fn jig_preserves_active_root_through_exact_graph_narrowing() {
    let _serial = JIG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let checkout = TempCheckout::new("jig-exact-graph-narrowing");
    checkout.init_git();
    seed_route_target(&checkout);
    let mcp = fake_mcp();
    let fake = FakeLlm::start(Script::rule(exact_narrowing_reply))
        .expect("start exact narrowing fake LLM");
    let provider = provider(&fake, "jig-exact-graph-narrowing");
    let config = tool_config(&mcp);
    let context = workspace_context();
    let cwd = checkout.path().to_path_buf();

    let result = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_coding_agent_native_with_tool_config(
            handle,
            &provider,
            &context,
            &cwd,
            12,
            None,
            Some(&config),
        )
        .await
    })
    .expect("exact narrowing route completes");

    assert_eq!(result.verdict, None);
    assert_eq!(
        fs::read_to_string(checkout.repo_path().join("ROUTE.md")).expect("narrowing product"),
        "exact graph narrowing preserved the route\n"
    );
    let calls = graph_calls(mcp.path());
    assert_eq!(
        tool_names(&calls),
        [
            "search_graph",
            "search_graph",
            "search_graph",
            "get_code_snippet",
            "trace_path",
            "get_code_snippet",
            "search_graph",
            "get_code_snippet",
        ]
    );
    assert_eq!(calls[2]["arguments"]["name_pattern"], "worker_slot");
    assert_eq!(calls[2]["arguments"]["label"], "Function");
    assert_eq!(
        calls[3]["arguments"]["qualified_name"],
        PARTIAL_IMPLEMENTATION
    );
}

fn exact_narrowing_reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0 => tool_reply(
            "broad-task-semantic-search",
            "codebase_memory_search_graph",
            serde_json::json!({
                "query": "alias retries preserve affinity through worker selection"
            }),
        ),
        1 => tool_reply(
            "narrow-task-semantic-search",
            "codebase_memory_search_graph",
            serde_json::json!({
                "query": "worker slot routes alias retry affinity"
            }),
        ),
        2 => tool_reply(
            "exact-symbol-and-type-search",
            "codebase_memory_search_graph",
            serde_json::json!({
                "name_pattern": "worker_slot",
                "label": "Function"
            }),
        ),
        3 => source_reply(
            "read-exact-narrowed-implementation",
            PARTIAL_IMPLEMENTATION.to_string(),
            "implementation",
        ),
        4 => tool_reply(
            "trace-exact-narrowed-implementation",
            "codebase_memory_trace_path",
            serde_json::json!({
                "function_name": PARTIAL_FUNCTION,
                "direction": "inbound"
            }),
        ),
        5 => source_reply("read-narrowed-caller", CALLER.to_string(), "caller"),
        6 => tool_reply(
            "search-focused-test-semantically",
            "codebase_memory_search_graph",
            serde_json::json!({
                "query": "request affinity remains stable across worker selection"
            }),
        ),
        7 => source_reply(
            "read-narrowed-focused-test",
            FOCUSED_TEST.to_string(),
            "focused_test",
        ),
        8 => tool_reply(
            "read-post-source-target",
            "read",
            serde_json::json!({"path": "demo/ROUTE.md"}),
        ),
        9 => tool_reply(
            "write-after-exact-read",
            "write",
            serde_json::json!({
                "path": "demo/ROUTE.md",
                "content": "exact graph narrowing preserved the route\n"
            }),
        ),
        10 => {
            Reply::text(r#"{"summary":"Preserved the active root through exact graph narrowing."}"#)
        }
        count => panic!("unexpected exact narrowing tool-result count {count}"),
    }
}
