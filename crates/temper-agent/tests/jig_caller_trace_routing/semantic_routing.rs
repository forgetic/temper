use super::*;

const SEMANTIC_IMPLEMENTATION: &str = "crate::scheduler::choose_lane";
const SEMANTIC_CALLER: &str = "crate::replay::route_renamed_job";
const SEMANTIC_FOCUSED_TEST: &str = "crate::tests::renamed_replay_preserves_shard_ownership";
const ACTIVE_ROOT_TEST: &str = "crate::tests::replay_uses_selected_lane";
const INCIDENTAL_IMPLEMENTATION: &str = "crate::routing::legacy_dispatch_key";
const BEHAVIOR_QUERY: &str = "renamed job replay shard ownership";
const REGRESSION_QUERY: &str = "renamed replay shard ownership regression";
const INCIDENTAL_IDENTIFIER: &str = "dispatch_key";

#[test]
fn jig_prefers_task_semantics_without_merging_identifier_lineage() {
    let _serial = JIG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let checkout = TempCheckout::new("jig-task-semantic-graph-route");
    checkout.init_git();
    let mcp = fake_mcp();
    let fake = FakeLlm::start(Script::rule(semantic_routing_reply))
        .expect("start task-semantic routing fake LLM");
    let provider = provider(&fake, "jig-task-semantic-graph-route");
    let config = tool_config(&mcp);
    let context = semantic_workspace_context();
    let cwd = checkout.path().to_path_buf();

    let result = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_coding_agent_native_with_tool_config(
            handle,
            &provider,
            &context,
            &cwd,
            14,
            None,
            Some(&config),
        )
        .await
    })
    .expect("task-semantic route completes on one root");

    assert_eq!(result.verdict, None);
    assert_eq!(
        fs::read_to_string(checkout.repo_path().join("SEMANTIC_ROUTE.md"))
            .expect("semantic routing product"),
        "task-semantic route retained\n"
    );
    assert!(
        !checkout.repo_path().join("PREMATURE.md").exists(),
        "typed evidence split across roots must not authorize mutation"
    );

    let calls = graph_calls(mcp.path());
    assert_eq!(calls.len(), 10);
    assert_eq!(calls[0]["name"], "search_graph");
    assert_eq!(calls[0]["arguments"]["query"], BEHAVIOR_QUERY);
    assert!(calls[0]["arguments"].get("name_pattern").is_none());
    assert_eq!(calls[1]["name"], "get_code_snippet");
    assert_eq!(
        calls[1]["arguments"]["qualified_name"],
        SEMANTIC_IMPLEMENTATION
    );
    assert_eq!(calls[2]["name"], "search_code");
    assert_eq!(calls[2]["arguments"]["pattern"], SEMANTIC_IMPLEMENTATION);
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["name"] == "search_graph")
            .count(),
        3
    );
    assert!(calls.iter().any(|call| {
        call["name"] == "search_graph" && call["arguments"]["name_pattern"] == INCIDENTAL_IDENTIFIER
    }));
    assert!(calls.iter().any(|call| {
        call["name"] == "search_graph" && call["arguments"]["query"] == REGRESSION_QUERY
    }));
    assert!(calls.iter().any(|call| {
        call["name"] == "trace_path"
            && call["arguments"]["function_name"] == SEMANTIC_IMPLEMENTATION
    }));
    for expected in [
        SEMANTIC_FOCUSED_TEST,
        INCIDENTAL_IMPLEMENTATION,
        SEMANTIC_CALLER,
        ACTIVE_ROOT_TEST,
    ] {
        assert!(calls.iter().any(|call| {
            call["name"] == "get_code_snippet" && call["arguments"]["qualified_name"] == expected
        }));
    }
}

fn semantic_routing_reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0 => {
            for expected in [
                "Renamed jobs must retain shard ownership during replay",
                "dispatch_key",
                "derive query",
                "task-semantic graph query",
                "behavioral regression",
            ] {
                assert!(
                    messages_contain(view, expected),
                    "prompt omitted {expected}"
                );
            }
            tool_reply(
                "discover-from-task-behavior",
                "codebase_memory_search_graph",
                serde_json::json!({"query": BEHAVIOR_QUERY}),
            )
        }
        1 => source_reply(
            "read-semantic-implementation",
            semantic_implementation_target(view),
            "implementation",
        ),
        2 => tool_reply(
            "refine-returned-implementation",
            "codebase_memory_search_code",
            serde_json::json!({
                "query": BEHAVIOR_QUERY,
                "pattern": semantic_implementation_target(view)
            }),
        ),
        3 => tool_batch(&[
            (
                "retain-typed-identifier-route",
                "codebase_memory_search_graph",
                serde_json::json!({"name_pattern": INCIDENTAL_IDENTIFIER}),
            ),
            (
                "discover-focused-test-semantically",
                "codebase_memory_search_graph",
                serde_json::json!({"query": REGRESSION_QUERY}),
            ),
        ]),
        5 => trace_reply(
            "trace-semantic-implementation",
            semantic_implementation_target(view),
        ),
        6 => source_reply(
            "read-semantically-returned-test",
            semantic_focused_test_target(view),
            "focused_test",
        ),
        7 => source_reply(
            "read-typed-incidental-implementation",
            incidental_implementation_target(view),
            "implementation",
        ),
        8 => source_reply(
            "read-traced-semantic-caller",
            semantic_caller_target(view),
            "caller",
        ),
        9 => tool_reply(
            "mutation-before-one-root-completes",
            "write",
            serde_json::json!({
                "path": "demo/PREMATURE.md",
                "content": "must remain blocked\n"
            }),
        ),
        10 => {
            assert!(messages_contain(view, "workspace mutation blocked until"));
            source_reply(
                "complete-active-root-focused-test",
                active_root_test_target(view),
                "focused_test",
            )
        }
        11 => tool_reply(
            "write-after-task-semantic-root-completes",
            "write",
            serde_json::json!({
                "path": "demo/SEMANTIC_ROUTE.md",
                "content": "task-semantic route retained\n"
            }),
        ),
        12 => Reply::text(
            r#"{"summary":"Preferred task-semantic evidence and kept identifier lineage separate."}"#,
        ),
        count => panic!("unexpected semantic routing tool-result count {count}"),
    }
}

fn returned_target(view: &RequestView, expected: &str) -> String {
    provider_results(view)
        .iter()
        .filter_map(|result| result.get("results").and_then(JsonValue::as_array))
        .flatten()
        .find_map(|result| {
            (result.get("qualified_name").and_then(JsonValue::as_str) == Some(expected))
                .then(|| expected.to_string())
        })
        .unwrap_or_else(|| panic!("provider did not return expected target {expected}"))
}

fn semantic_implementation_target(view: &RequestView) -> String {
    returned_target(view, SEMANTIC_IMPLEMENTATION)
}

fn incidental_implementation_target(view: &RequestView) -> String {
    returned_target(view, INCIDENTAL_IMPLEMENTATION)
}

fn semantic_focused_test_target(view: &RequestView) -> String {
    returned_target(view, SEMANTIC_FOCUSED_TEST)
}

fn active_root_test_target(view: &RequestView) -> String {
    returned_target(view, ACTIVE_ROOT_TEST)
}

fn semantic_caller_target(view: &RequestView) -> String {
    provider_results(view)
        .into_iter()
        .find(|result| {
            result
                .pointer("/function/qualified_name")
                .and_then(JsonValue::as_str)
                == Some(SEMANTIC_IMPLEMENTATION)
        })
        .and_then(|result| {
            result
                .pointer("/callers/0/qualified_name")
                .and_then(JsonValue::as_str)
                .map(str::to_string)
        })
        .expect("semantic implementation trace returned its relevant caller")
}

fn semantic_workspace_context() -> WorkspaceContext {
    let mut context = workspace_context();
    context.work_item.context = serde_json::json!({
        "artifact": {
            "type": "issue",
            "number": 25,
            "title": "Keep renamed replay work on its original shard",
            "body": "Renamed jobs must retain shard ownership during replay. The report mentions the dispatch_key accessor, but repair the routing behavior and add regression coverage.",
            "labels": ["code", "ready"],
            "state": "Open"
        }
    })
    .to_string();
    context
}
