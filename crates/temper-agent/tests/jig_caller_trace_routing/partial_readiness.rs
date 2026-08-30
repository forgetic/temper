use super::*;

#[test]
fn jig_defers_partial_implementation_traversal_then_recovers_malformed_selector() {
    let _serial = JIG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let checkout = TempCheckout::new("jig-caller-trace-partial-readiness");
    checkout.init_git();
    seed_route_target(&checkout);
    let mcp = fake_mcp();
    let fake = FakeLlm::start(Script::rule(partial_readiness_reply))
        .expect("start partial-readiness fake LLM");
    let provider = provider(&fake, "jig-caller-trace-partial-readiness");
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
    .expect("partial traversal readiness recovers locally");

    assert_eq!(result.verdict, None);
    assert_eq!(
        fs::read_to_string(checkout.repo_path().join("ROUTE.md")).expect("recovery product"),
        "partial traversal recovered locally\n"
    );
    let calls = graph_calls(mcp.path());
    assert_eq!(
        tool_names(&calls),
        [
            "search_code",
            "get_code_snippet",
            "trace_path",
            "get_code_snippet",
            "search_graph",
            "get_code_snippet",
        ],
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["name"] == "trace_path")
            .count(),
        1,
        "only the recognized live reference may dispatch a trace",
    );
    assert_eq!(
        calls[1]["arguments"]["qualified_name"],
        PARTIAL_IMPLEMENTATION
    );
    assert_eq!(calls[2]["arguments"]["function_name"], PARTIAL_FUNCTION);
    assert_eq!(calls[2]["arguments"]["direction"], "inbound");
    assert!(
        !serde_json::to_string(&calls)
            .expect("provider calls serialize")
            .contains("temper-recovery-selector:"),
        "the provider call log must never receive an opaque reference",
    );
}

fn partial_readiness_reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0 => tool_reply(
            "discover-partial-implementation",
            "codebase_memory_search_code",
            serde_json::json!({
                "query": "worker selection",
                "pattern": "partial select worker"
            }),
        ),
        1 => source_reply(
            "read-partial-implementation",
            implementation_target(view),
            "implementation",
        ),
        2 => tool_reply(
            "trace-with-malformed-selector",
            "codebase_memory_trace_path",
            serde_json::json!("inbound"),
        ),
        3 => {
            let latest_tool = view
                .messages
                .iter()
                .rev()
                .find(|message| message.role == "tool")
                .expect("local malformed traversal denial returned a tool result");
            assert!(
                latest_tool
                    .content
                    .contains("decision-evidence recovery required")
            );
            assert!(latest_tool.content.contains("remaining allowance: 4"));
            assert!(messages_contain(
                view,
                "required next call=[codebase_memory_trace_path"
            ));
            trace_reply(
                "trace-with-live-recovery-reference",
                implementation_recovery_reference(view),
            )
        }
        4 => source_reply("read-partial-caller", caller_relationship(view), "caller"),
        5 => tool_reply(
            "search-partial-focused-test",
            "codebase_memory_search_graph",
            serde_json::json!({
                "query": "request affinity remains stable across worker selection"
            }),
        ),
        6 => source_reply(
            "read-partial-focused-test",
            semantic_test_relationship(view),
            "focused_test",
        ),
        7 => tool_reply(
            "read-partial-target",
            "read",
            serde_json::json!({"path": "demo/ROUTE.md"}),
        ),
        8 => tool_reply(
            "write-partial-product",
            "write",
            serde_json::json!({
                "path": "demo/ROUTE.md",
                "content": "partial traversal recovered locally\n"
            }),
        ),
        9 => Reply::text(
            r#"{"summary":"Consumed the live recovery reference and completed typed caller and focused-test evidence."}"#,
        ),
        count => panic!("unexpected partial-readiness tool-result count {count}"),
    }
}

fn implementation_recovery_reference(view: &RequestView) -> String {
    view.messages
        .iter()
        .rev()
        .filter(|message| message.role == "tool")
        .find_map(|message| {
            message
                .content
                .split_once("implementation_evidence_result=")
                .and_then(|(_, rest)| rest.split([',', '.']).next())
                .map(str::to_string)
        })
        .expect("implementation tool output returned a live opaque trace reference")
}
