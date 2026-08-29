use super::*;

#[test]
fn jig_defers_partial_implementation_traversal_then_completes_typed_evidence() {
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
        "the immediate traversal over partial caller metadata must stay local",
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
        2 => trace_reply(
            "trace-partial-immediately",
            implementation_recovery_reference(view),
        ),
        3 => {
            let latest_tool = view
                .messages
                .iter()
                .rev()
                .find(|message| message.role == "tool")
                .expect("local readiness denial returned a tool result");
            assert!(
                latest_tool
                    .content
                    .contains("decision-evidence recovery required")
            );
            assert!(messages_contain(view, "Traversal readiness:"));
            assert!(messages_contain(
                view,
                "no provider call or recovery allowance was consumed"
            ));
            assert!(messages_contain(
                view,
                "required next stage=[get_code_snippet/qualified_name/implementation]"
            ));
            source_reply(
                "refresh-partial-implementation",
                implementation_recovery_reference(view),
                "implementation",
            )
        }
        4 => trace_reply(
            "trace-after-provider-enrichment",
            implementation_recovery_reference(view),
        ),
        5 => source_reply("read-partial-caller", caller_relationship(view), "caller"),
        6 => tool_reply(
            "search-partial-focused-test",
            "codebase_memory_search_graph",
            serde_json::json!({
                "query": "request affinity remains stable across worker selection"
            }),
        ),
        7 => source_reply(
            "read-partial-focused-test",
            semantic_test_relationship(view),
            "focused_test",
        ),
        8 => tool_reply(
            "read-partial-target",
            "read",
            serde_json::json!({"path": "demo/ROUTE.md"}),
        ),
        9 => tool_reply(
            "write-partial-product",
            "write",
            serde_json::json!({
                "path": "demo/ROUTE.md",
                "content": "partial traversal recovered locally\n"
            }),
        ),
        10 => Reply::text(
            r#"{"summary":"Deferred partial traversal and completed typed caller and focused-test evidence."}"#,
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
        .expect("partial implementation returned an opaque traversal reference")
}
