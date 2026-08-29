use super::*;

const FALLBACK_QUERY: &str = "request affinity remains stable across worker selection";
const AMBIGUOUS_FALLBACK_QUERY: &str = "request affinity regression with ambiguous coverage";
const MALFORMED_FALLBACK_QUERY: &str = "request affinity regression with malformed coverage";
const NON_TEST_FALLBACK_QUERY: &str = "request affinity regression returning a non-test helper";

#[test]
fn jig_empty_traversal_uses_one_semantic_fallback_before_mutation() {
    let _serial = JIG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let checkout = TempCheckout::new("jig-focused-test-semantic-fallback");
    checkout.init_git();
    seed_route_target(&checkout);
    let mcp = fake_mcp();
    let fake = FakeLlm::start(Script::rule(fallback_success_reply))
        .expect("start focused-test fallback fake LLM");
    let provider = provider(&fake, "jig-focused-test-semantic-fallback");
    let config = tool_config(&mcp);
    let context = fallback_workspace_context();
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
    .expect("one same-root semantic fallback completes focused-test evidence");

    assert_eq!(result.verdict, None);
    assert_eq!(
        fs::read_to_string(checkout.repo_path().join("ROUTE.md")).expect("fallback product"),
        "semantic fallback verified\n"
    );
    let calls = graph_calls(mcp.path());
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["name"] == "search_graph")
            .count(),
        1,
        "exactly one fallback search reaches the provider"
    );
    assert!(calls.iter().any(|call| {
        call["name"] == "search_graph" && call["arguments"]["query"] == FALLBACK_QUERY
    }));
    assert_eq!(
        calls
            .iter()
            .filter(|call| {
                call["name"] == "trace_path"
                    && call["arguments"].get("include_tests") == Some(&serde_json::json!(true))
            })
            .count(),
        0,
        "the deterministic route must not detour through a caller-to-test traversal",
    );
}

#[test]
fn jig_ambiguous_malformed_and_non_test_fallbacks_stop_without_retry_or_product() {
    let _serial = JIG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (case, reply) in [
        (
            "ambiguous",
            fallback_ambiguous_reply as fn(&RequestView) -> Reply,
        ),
        ("malformed", fallback_malformed_reply),
        ("non-test", fallback_non_test_reply),
    ] {
        assert_fallback_exhaustion(case, reply);
    }
}

fn fallback_success_reply(view: &RequestView) -> Reply {
    fallback_reply(view, FALLBACK_QUERY, true)
}

fn fallback_ambiguous_reply(view: &RequestView) -> Reply {
    fallback_reply(view, AMBIGUOUS_FALLBACK_QUERY, false)
}

fn fallback_malformed_reply(view: &RequestView) -> Reply {
    fallback_reply(view, MALFORMED_FALLBACK_QUERY, false)
}

fn fallback_non_test_reply(view: &RequestView) -> Reply {
    fallback_reply(view, NON_TEST_FALLBACK_QUERY, true)
}

fn assert_fallback_exhaustion(case: &str, reply: fn(&RequestView) -> Reply) {
    let checkout = TempCheckout::new(&format!("jig-focused-test-fallback-{case}"));
    checkout.init_git();
    let mcp = fake_mcp();
    let fake = FakeLlm::start(Script::rule(reply)).expect("start focused-test exhaustion fake LLM");
    let provider = provider(&fake, &format!("jig-focused-test-fallback-{case}"));
    let config = tool_config(&mcp);
    let context = fallback_workspace_context();
    let cwd = checkout.path().to_path_buf();

    let error = temper_agent_io::block_on_with(move |_cx, handle| async move {
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
    .expect_err("inexact fallback must stop without a product");
    assert!(matches!(
        error,
        CodingAgentError::DecisionAnchorRecoveryExhausted | CodingAgentError::NoProduct
    ));
    assert!(
        !checkout
            .repo_path()
            .join("FOCUSED_TEST_FALLBACK.md")
            .exists()
    );

    let calls = graph_calls(mcp.path());
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["name"] == "search_graph")
            .count(),
        1,
        "exhaustion must not retry the fallback"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| {
                call["name"] == "trace_path"
                    && call["arguments"].get("include_tests") == Some(&serde_json::json!(true))
            })
            .count(),
        0,
        "exhaustion must not detour through a traversal"
    );
}

fn fallback_reply(view: &RequestView, query: &str, succeeds: bool) -> Reply {
    match view.prior_tool_results {
        0 => {
            for expected in [
                "Initial task-semantic discovery may over-return",
                "one same-root",
                "focused-test search in the next turn",
                "never copy a fixture or test name",
                "without retrying or inventing a selector",
            ] {
                assert!(
                    messages_contain(view, expected),
                    "prompt omitted {expected}"
                );
            }
            search_reply("discover-fallback-implementation")
        }
        1 => source_reply(
            "read-fallback-implementation",
            implementation_target(view),
            "implementation",
        ),
        2 => trace_reply("trace-fallback-implementation", implementation_target(view)),
        3 => source_reply("read-fallback-caller", caller_relationship(view), "caller"),
        4 => {
            for expected in [
                "search_graph/graph_query/focused_test",
                "selector=task_semantic_query",
            ] {
                assert!(
                    messages_contain(view, expected),
                    "semantic search menu omitted {expected}"
                );
            }
            tool_reply(
                "one-semantic-focused-test-search",
                "codebase_memory_search_graph",
                serde_json::json!({"query": query}),
            )
        }
        5 if !succeeds => Reply::text(
            r#"{"summary":"Stopped after the semantic search returned no exact focused test."}"#,
        ),
        5 if succeeds => source_reply(
            "read-search-returned-test",
            fallback_test_target(view),
            "focused_test",
        ),
        6 if succeeds => tool_reply(
            "read-fallback-target-after-evidence",
            "read",
            serde_json::json!({"path": "demo/ROUTE.md"}),
        ),
        7 if succeeds => tool_reply(
            "write-after-fallback-evidence",
            "write",
            serde_json::json!({
                "path": "demo/ROUTE.md",
                "content": "semantic fallback verified\n"
            }),
        ),
        8 if succeeds => Reply::text(
            r#"{"summary":"Used one same-root semantic search and its exact returned test."}"#,
        ),
        count => panic!("unexpected focused-test search tool-result count {count}"),
    }
}

fn fallback_test_target(view: &RequestView) -> String {
    provider_results(view)
        .iter()
        .rev()
        .filter_map(|result| result.get("results").and_then(JsonValue::as_array))
        .flatten()
        .find_map(|result| result.get("qualified_name").and_then(JsonValue::as_str))
        .map(str::to_string)
        .expect("semantic fallback returned one exact test")
}

fn fallback_workspace_context() -> WorkspaceContext {
    let mut context = workspace_context();
    context.work_item.context = serde_json::json!({
        "artifact": {
            "type": "issue",
            "number": 25,
            "title": "Keep request affinity stable",
            "body": "Requests must keep affinity while workers are selected. Recover focused behavioral coverage before changing the route.",
            "labels": ["code", "ready"],
            "state": "Open"
        }
    })
    .to_string();
    context
}
