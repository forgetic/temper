use super::*;

const UNKNOWN_SELECTOR: &str = "crate::never_returned::missing_selector";

#[test]
fn jig_denies_unknown_trace_locally_then_recovers_exact_selector() {
    let _serial = JIG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let checkout = TempCheckout::new("jig-caller-trace-unknown-selector");
    checkout.init_git();
    seed_route_target(&checkout);
    let mcp = fake_mcp();
    let fake =
        FakeLlm::start(Script::rule(unknown_trace_reply)).expect("start unknown trace fake LLM");
    let provider = provider(&fake, "jig-caller-trace-unknown-selector");
    let config = tool_config(&mcp);
    let context = workspace_context();
    let cwd = checkout.path().to_path_buf();

    let result = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_coding_agent_native_with_tool_config(
            handle,
            &provider,
            &context,
            &cwd,
            11,
            None,
            Some(&config),
        )
        .await
    })
    .expect("unknown-selector recovery route completes");

    assert_eq!(result.verdict, None);
    assert_eq!(
        fs::read_to_string(checkout.repo_path().join("ROUTE.md")).expect("recovery product"),
        "unknown trace recovered locally\n"
    );
    let calls = graph_calls(mcp.path());
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["name"] == "trace_path")
            .count(),
        1,
        "a traversal selector absent from every root must not reach the provider"
    );
    let trace = calls
        .iter()
        .find(|call| call["name"] == "trace_path")
        .expect("recovered traversal reached provider");
    assert_eq!(trace["arguments"]["function_name"], IMPLEMENTATION);
    assert_eq!(trace["arguments"]["direction"], "inbound");
}

fn unknown_trace_reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0 => search_reply("discover-unknown-trace-implementation"),
        1 => source_reply(
            "read-unknown-trace-implementation",
            implementation_target(view),
            "implementation",
        ),
        2 => tool_reply(
            "trace-with-unknown-selector",
            "codebase_memory_trace_path",
            serde_json::json!({
                "function_name": UNKNOWN_SELECTOR,
                "direction": "inbound"
            }),
        ),
        3 => {
            let latest_tool = view
                .messages
                .iter()
                .rev()
                .find(|message| message.role == "tool")
                .expect("local traversal denial returned a tool result");
            assert!(
                latest_tool
                    .content
                    .contains("decision-evidence recovery required")
            );
            assert!(latest_tool.content.contains("remaining allowance: 4"));
            for expected in [
                "required next stage=[trace_path/function_name/trace/",
                "selector=implementation_evidence_result",
                "direction=inbound",
            ] {
                assert!(
                    messages_contain(view, expected),
                    "guidance omitted {expected}"
                );
            }
            trace_reply(
                "trace-with-recovered-function-name",
                implementation_target(view),
            )
        }
        4 => source_reply(
            "read-unknown-trace-caller",
            caller_relationship(view),
            "caller",
        ),
        5 => tool_reply(
            "search-unknown-trace-focused-test",
            "codebase_memory_search_graph",
            serde_json::json!({
                "query": "request affinity remains stable across worker selection"
            }),
        ),
        6 => source_reply(
            "read-unknown-trace-focused-test",
            semantic_test_relationship(view),
            "focused_test",
        ),
        7 => tool_reply(
            "read-unknown-trace-target",
            "read",
            serde_json::json!({"path": "demo/ROUTE.md"}),
        ),
        8 => tool_reply(
            "write-unknown-trace-product",
            "write",
            serde_json::json!({
                "path": "demo/ROUTE.md",
                "content": "unknown trace recovered locally\n"
            }),
        ),
        9 => Reply::text(
            r#"{"summary":"Recovered the unknown traversal with the exact returned selector."}"#,
        ),
        count => panic!("unexpected unknown trace tool-result count {count}"),
    }
}
