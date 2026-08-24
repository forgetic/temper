use super::*;

const SEMANTIC_IMPLEMENTATION: &str = "crate::scheduler::choose_lane";
const SEMANTIC_CALLER: &str = "crate::replay::route_renamed_job";
const SEMANTIC_FOCUSED_TEST: &str = "crate::tests::renamed_replay_preserves_shard_ownership";
const ACTIVE_ROOT_TEST: &str = "crate::tests::replay_uses_selected_lane";
const BEHAVIOR_QUERY: &str = "renamed job replay shard ownership";
const REGRESSION_QUERY: &str = "renamed replay shard ownership regression";

#[test]
fn jig_stages_over_returned_candidates_through_caller_and_test_routes() {
    let _serial = JIG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let checkout = TempCheckout::new("jig-staged-over-returned-evidence");
    checkout.init_git();
    let mcp = fake_mcp();
    let fake = FakeLlm::start(Script::rule(semantic_routing_reply))
        .expect("start staged routing fake LLM");
    let provider = provider(&fake, "jig-staged-over-returned-evidence");
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
    });
    let result = result.expect("staged caller and focused-test routes complete one root");

    assert_eq!(result.verdict, None);
    assert_eq!(
        fs::read_to_string(checkout.repo_path().join("SEMANTIC_ROUTE.md"))
            .expect("semantic routing product"),
        "task-semantic route retained\n"
    );
    assert!(
        !checkout.repo_path().join("PREMATURE.md").exists(),
        "direct reads of over-returned caller and test candidates must not authorize mutation"
    );

    let calls = graph_calls(mcp.path());
    assert_eq!(
        tool_names(&calls),
        [
            "search_graph",
            "get_code_snippet",
            "get_code_snippet",
            "get_code_snippet",
            "trace_path",
            "get_code_snippet",
            "trace_path",
            "search_graph",
            "get_code_snippet",
        ]
    );
    assert_eq!(calls[0]["arguments"]["query"], BEHAVIOR_QUERY);
    assert_eq!(
        calls[1]["arguments"]["qualified_name"],
        SEMANTIC_IMPLEMENTATION
    );
    assert_eq!(calls[2]["arguments"]["qualified_name"], SEMANTIC_CALLER);
    assert_eq!(calls[3]["arguments"]["qualified_name"], ACTIVE_ROOT_TEST);
    assert_eq!(
        calls[4]["arguments"]["function_name"],
        SEMANTIC_IMPLEMENTATION
    );
    assert_eq!(calls[5]["arguments"]["qualified_name"], SEMANTIC_CALLER);
    assert_eq!(calls[6]["arguments"]["function_name"], SEMANTIC_CALLER);
    assert_eq!(calls[6]["arguments"]["include_tests"], true);
    assert_eq!(calls[7]["arguments"]["query"], REGRESSION_QUERY);
    assert_eq!(
        calls[8]["arguments"]["qualified_name"],
        SEMANTIC_FOCUSED_TEST
    );
}

fn semantic_routing_reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0 => {
            for expected in [
                "implementation-purpose result may over-return",
                "selected implementation source first",
                "exact identity returned by that traversal",
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
            "read-selected-implementation",
            returned_target(view, SEMANTIC_IMPLEMENTATION),
            "implementation",
        ),
        2 => source_reply(
            "direct-over-returned-caller",
            returned_target(view, SEMANTIC_CALLER),
            "caller",
        ),
        3 => source_reply(
            "direct-over-returned-test",
            returned_target(view, ACTIVE_ROOT_TEST),
            "focused_test",
        ),
        4 => tool_reply(
            "mutation-before-staged-routes",
            "write",
            serde_json::json!({
                "path": "demo/PREMATURE.md",
                "content": "must remain blocked\n"
            }),
        ),
        5 => {
            assert!(messages_contain(view, "workspace mutation blocked until"));
            trace_reply(
                "trace-selected-implementation",
                returned_target(view, SEMANTIC_IMPLEMENTATION),
            )
        }
        6 => source_reply(
            "read-traversal-returned-caller",
            traced_caller_target(view),
            "caller",
        ),
        7 => test_trace_reply("traverse-caller-for-tests", traced_caller_target(view)),
        8 => tool_reply(
            "one-task-semantic-test-fallback",
            "codebase_memory_search_graph",
            serde_json::json!({"query": REGRESSION_QUERY}),
        ),
        9 => source_reply(
            "read-fallback-returned-test",
            returned_target(view, SEMANTIC_FOCUSED_TEST),
            "focused_test",
        ),
        10 => tool_reply(
            "write-after-staged-evidence",
            "write",
            serde_json::json!({
                "path": "demo/SEMANTIC_ROUTE.md",
                "content": "task-semantic route retained\n"
            }),
        ),
        11 => Reply::text(
            r#"{"summary":"Staged over-returned candidates through exact caller and focused-test routes."}"#,
        ),
        count => panic!("unexpected staged routing tool-result count {count}"),
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

fn traced_caller_target(view: &RequestView) -> String {
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
        .expect("selected implementation traversal returned its exact caller")
}

fn semantic_workspace_context() -> WorkspaceContext {
    let mut context = workspace_context();
    context.work_item.context = serde_json::json!({
        "artifact": {
            "type": "issue",
            "number": 25,
            "title": "Keep renamed replay work on its original shard",
            "body": "Renamed jobs must retain shard ownership during replay and preserve focused behavioral coverage.",
            "labels": ["code", "ready"],
            "state": "Open"
        }
    })
    .to_string();
    context
}
