use super::*;

const FALLBACK_QUERY: &str = "request affinity remains stable across worker selection";
const EMPTY_FALLBACK_QUERY: &str = "request affinity regression without a matching test";

#[test]
fn jig_empty_traversal_uses_one_semantic_fallback_before_mutation() {
    let _serial = JIG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let checkout = TempCheckout::new("jig-focused-test-semantic-fallback");
    checkout.init_git();
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
        fs::read_to_string(checkout.repo_path().join("FOCUSED_TEST_FALLBACK.md"))
            .expect("fallback product"),
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
        1,
    );
}

#[test]
fn jig_empty_traversal_and_fallback_stop_without_retry_or_product() {
    let _serial = JIG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let checkout = TempCheckout::new("jig-focused-test-fallback-exhaustion");
    checkout.init_git();
    let mcp = fake_mcp();
    let fake = FakeLlm::start(Script::rule(fallback_exhaustion_reply))
        .expect("start focused-test exhaustion fake LLM");
    let provider = provider(&fake, "jig-focused-test-fallback-exhaustion");
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
    .expect_err("empty traversal and fallback must stop without a product");
    assert!(matches!(
        error,
        CodingAgentError::DecisionAnchorRecoveryExhausted
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
        1,
        "exhaustion must not retry the traversal"
    );
}

fn fallback_success_reply(view: &RequestView) -> Reply {
    fallback_reply(view, FALLBACK_QUERY, true)
}

fn fallback_exhaustion_reply(view: &RequestView) -> Reply {
    fallback_reply(view, EMPTY_FALLBACK_QUERY, false)
}

fn fallback_reply(view: &RequestView, query: &str, succeeds: bool) -> Reply {
    match view.prior_tool_results {
        0 => {
            for expected in [
                "complete traversal returns no eligible test identity",
                "task intent",
                "never copy a fixture or test name",
                "stop without a product if both routes return no eligible test",
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
        4 => refinement_reply("non-progressing-fallback-one", implementation_target(view)),
        5 => refinement_reply("non-progressing-fallback-two", implementation_target(view)),
        6 => {
            assert!(messages_contain(view, "selector=caller_evidence_result"));
            tool_reply(
                "empty-focused-test-traversal",
                "codebase_memory_trace_path",
                serde_json::json!({
                    "function_name": caller_relationship(view),
                    "mode": "calls",
                    "direction": "inbound",
                    "include_tests": true,
                    "depth": 1
                }),
            )
        }
        7 => {
            for expected in [
                "search_graph/graph_query/focused_test",
                "selector=task_semantic_query",
                "use only these current-root actions",
            ] {
                assert!(
                    messages_contain(view, expected),
                    "fallback menu omitted {expected}"
                );
            }
            tool_reply(
                "one-semantic-focused-test-fallback",
                "codebase_memory_search_graph",
                serde_json::json!({"query": query}),
            )
        }
        8 if succeeds => source_reply(
            "read-fallback-returned-test",
            fallback_test_target(view),
            "focused_test",
        ),
        9 if succeeds => tool_reply(
            "write-after-fallback-evidence",
            "write",
            serde_json::json!({
                "path": "demo/FOCUSED_TEST_FALLBACK.md",
                "content": "semantic fallback verified\n"
            }),
        ),
        10 if succeeds => Reply::text(
            r#"{"summary":"Used one same-root semantic fallback and its exact returned test."}"#,
        ),
        count => panic!("unexpected focused-test fallback tool-result count {count}"),
    }
}

fn fallback_test_target(view: &RequestView) -> String {
    provider_results(view)
        .iter()
        .filter_map(|result| result.get("results").and_then(JsonValue::as_array))
        .flatten()
        .find_map(|result| {
            (result.get("is_test").and_then(JsonValue::as_bool) == Some(true))
                .then(|| result.get("qualified_name").and_then(JsonValue::as_str))
                .flatten()
                .map(str::to_string)
        })
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
