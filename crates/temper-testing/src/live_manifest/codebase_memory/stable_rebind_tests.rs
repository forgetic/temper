use std::fs;

use super::*;

fn call(
    name: &str,
    arguments: JsonValue,
    delay_ms: Option<u64>,
    is_error: bool,
    fixture_event: Option<&str>,
) -> McpToolCallEvidence {
    McpToolCallEvidence {
        name: name.to_string(),
        arguments,
        delay_ms,
        is_error,
        fixture_event: fixture_event.map(str::to_string),
    }
}

#[test]
fn contract_requires_normalized_ready_confirmation_before_confirmed_reads() {
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("mcp.jsonl");
    let state_path = workspace.path().join("mcp.jsonl.state.json");
    let requested = "temper-v1-demo";
    let confirmed = "normalized-temper-v1-demo";
    fs::write(
        &state_path,
        serde_json::json!({
            "projects": {
                confirmed: {
                    "requested_stable_project": requested,
                    "repo_path": "/workspace/demo",
                    "binding": "current_prepared_checkout"
                }
            },
            "counters": {"project_creations": 1, "rebinds": 1}
        })
        .to_string(),
    )
    .expect("write provider state");
    let mcp = FakeMcpServer {
        script_path: workspace.path().join("fake.py"),
        log_path,
        state_path,
        project: "demo".to_string(),
        lifecycle_profile: Some("stable-rebind".to_string()),
        safe_tools: vec!["search_graph".to_string(), "get_code_snippet".to_string()],
        hidden_tools: vec!["index_repository".to_string()],
        readiness_delay_ms: 750,
        forced_systemic_failure: Some(super::super::ForcedSystemicFailureFixture {
            tool: "search_graph".to_string(),
            after_calls: 1,
        }),
    };
    let calls = vec![
        call(
            "index_status",
            serde_json::json!({"project": requested}),
            None,
            false,
            Some("fresh_prior_binding"),
        ),
        call(
            "index_repository",
            serde_json::json!({"name": requested, "repo_path": "/workspace/demo"}),
            Some(750),
            false,
            Some("normalized_current_root_upsert"),
        ),
        call(
            "index_status",
            serde_json::json!({"project": confirmed}),
            None,
            false,
            Some("current_root_confirmed"),
        ),
        call(
            "search_graph",
            serde_json::json!({"project": confirmed}),
            None,
            false,
            Some("served_current_root_graph"),
        ),
        call(
            "get_code_snippet",
            serde_json::json!({"project": confirmed, "qualified_name": "retry_worker_topic"}),
            None,
            false,
            Some("served_current_root_source"),
        ),
        call(
            "search_graph",
            serde_json::json!({"project": confirmed}),
            None,
            true,
            None,
        ),
    ];

    let evidence = stable_rebind_evidence(&mcp, &calls)
        .expect("read stable rebind evidence")
        .expect("stable profile evidence");
    assert_eq!(evidence.requested_stable_project, requested);
    assert_eq!(evidence.confirmed_provider_project, confirmed);
    assert_eq!(evidence.confirmation_call_count, 2);
    assert!(evidence.normalized_provider_identity);
    assert!(evidence.graph_reads_use_confirmed_project);
    assert!(evidence.source_reads_use_confirmed_project);
    validate_stable_rebind_contract(&mcp, &calls, requested)
        .expect("exact confirmation inventory is accepted");
}

#[test]
fn graph_consumption_contract_requires_the_declared_current_root_chain() {
    let workspace = tempfile::tempdir().expect("workspace");
    let requested = "temper-v1-demo";
    let confirmed = "normalized-temper-v1-demo";
    let state_path = workspace.path().join("mcp.jsonl.state.json");
    fs::write(
        &state_path,
        serde_json::json!({
            "projects": {
                confirmed: {
                    "requested_stable_project": requested,
                    "repo_path": "/workspace/demo",
                    "binding": "current_prepared_checkout"
                }
            },
            "counters": {"project_creations": 1, "rebinds": 1},
            "historical_tokens": {"implementation":"retry_worker_topic", "caller":"dispatch", "focused_test":"alias_retries_keep_the_original_ordered_worker"}
        })
        .to_string(),
    )
    .expect("write provider state");
    let mcp = FakeMcpServer {
        script_path: workspace.path().join("fake.py"),
        log_path: workspace.path().join("mcp.jsonl"),
        state_path,
        project: "demo".to_string(),
        lifecycle_profile: Some("graph-consumption".to_string()),
        safe_tools: vec![
            "search_graph".to_string(),
            "search_code".to_string(),
            "trace_path".to_string(),
            "get_code_snippet".to_string(),
        ],
        hidden_tools: vec!["index_repository".to_string()],
        readiness_delay_ms: 750,
        forced_systemic_failure: None,
    };
    let calls = vec![
        call(
            "index_status",
            serde_json::json!({"project": requested}),
            None,
            false,
            Some("fresh_prior_binding"),
        ),
        call(
            "index_repository",
            serde_json::json!({"name": requested, "repo_path": "/workspace/demo"}),
            Some(750),
            false,
            Some("normalized_current_root_upsert"),
        ),
        call(
            "index_status",
            serde_json::json!({"project": confirmed}),
            None,
            false,
            Some("current_root_confirmed"),
        ),
        call(
            "search_graph",
            serde_json::json!({"project": confirmed, "query": "alias retry worker affinity"}),
            None,
            false,
            Some("served_current_root_graph"),
        ),
        call(
            "search_code",
            serde_json::json!({"project": confirmed, "pattern": "retry_worker_topic"}),
            None,
            false,
            Some("served_current_root_code_refinement"),
        ),
        call(
            "get_code_snippet",
            serde_json::json!({"project": confirmed, "qualified_name": "retry_worker_topic"}),
            None,
            false,
            Some("served_current_root_source"),
        ),
        call(
            "trace_path",
            serde_json::json!({"project": confirmed, "function_name": "retry_worker_topic", "direction":"inbound"}),
            None,
            false,
            Some("served_current_root_graph_trace"),
        ),
        call(
            "get_code_snippet",
            serde_json::json!({"project": confirmed, "qualified_name": "dispatch"}),
            None,
            false,
            Some("served_current_root_source"),
        ),
        call(
            "get_code_snippet",
            serde_json::json!({"project": confirmed, "qualified_name": "alias_retries_keep_the_original_ordered_worker"}),
            None,
            false,
            Some("served_current_root_source"),
        ),
    ];

    validate_mcp_contract(&mcp, &calls).expect("declared graph-consumption chain is accepted");
    let mut missing_source = calls.clone();
    missing_source.remove(7);
    assert!(validate_mcp_contract(&mcp, &missing_source).is_err());
    let mut wrong_selector = calls.clone();
    wrong_selector[7].arguments["qualified_name"] = serde_json::json!("retry_worker_topic");
    assert!(validate_mcp_contract(&mcp, &wrong_selector).is_err());
    let mut leaked_wrapper_purpose = calls;
    leaked_wrapper_purpose[5].arguments["decision_evidence_kind"] =
        serde_json::json!("implementation");
    assert!(validate_mcp_contract(&mcp, &leaked_wrapper_purpose).is_err());
}
