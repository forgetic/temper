use super::*;

#[test]
fn forge_side_channel_rejects_child_selected_identity_and_mutations() {
    let with_identity = serde_json::json!({
        "protocol_version": PROTOCOL_VERSION,
        "worker_id": "attacker-worker",
        "job_id": "other-job",
        "operation": {
            "operation": "forge_get_item",
            "repo": "ai/temper",
            "number": 284
        }
    });
    assert!(serde_json::from_value::<ForgeContextRequest>(with_identity).is_err());

    let mutation = serde_json::json!({
        "protocol_version": PROTOCOL_VERSION,
        "operation": {
            "operation": "close_issue",
            "repo": "ai/temper",
            "number": 284
        }
    });
    assert!(serde_json::from_value::<ForgeContextRequest>(mutation).is_err());
}

#[test]
fn codebase_memory_tool_config_round_trips_and_filters_roles() {
    let json = r#"{
        "codebase_memory": {
            "mode": "auto",
            "command": "codebase-memory-mcp",
            "args": ["--cache", "local"],
            "roles": ["engineer"],
            "index": "background",
            "startup_timeout_secs": 5,
            "index_timeout_secs": 30
        }
    }"#;
    let config = AgentToolConfig::from_json(json).expect("parse tool config");
    assert!(config.enabled_for_role("engineer"));
    assert!(!config.enabled_for_role("architect"));
    let rendered = config.to_json().expect("serialize tool config");
    assert_eq!(AgentToolConfig::from_json(&rendered).unwrap(), config);
}

#[test]
fn codebase_memory_tool_config_rejects_invalid_values() {
    for json in [
        r#"{"codebase_memory":{"mode":"auto","command":"","roles":["*"],"index":"background","startup_timeout_secs":5,"index_timeout_secs":30}}"#,
        r#"{"codebase_memory":{"mode":"auto","command":"cmd","roles":[""],"index":"background","startup_timeout_secs":5,"index_timeout_secs":30}}"#,
        r#"{"codebase_memory":{"mode":"auto","command":"cmd","roles":["*"],"index":"background","startup_timeout_secs":0,"index_timeout_secs":30}}"#,
        r#"{"codebase_memory":{"mode":"auto","command":"cmd","roles":["*"],"index":"eventually","startup_timeout_secs":5,"index_timeout_secs":30}}"#,
    ] {
        assert!(
            AgentToolConfig::from_json(json).is_err(),
            "invalid config should fail: {json}"
        );
    }
}

#[test]
fn api_key_credential_round_trips() {
    let credential = ProviderCredentialJson::from_json(r#"{"type":"api-key","api_key":"sk-x"}"#)
        .expect("parse api-key");
    assert_eq!(
        credential,
        ProviderCredentialJson::ApiKey {
            api_key: "sk-x".to_string(),
        }
    );
    let json = credential.to_json().expect("serialize");
    assert_eq!(
        ProviderCredentialJson::from_json(&json).expect("re-parse"),
        credential
    );
}

#[test]
fn oauth_credential_parses_with_and_without_refresh() {
    let with_refresh = ProviderCredentialJson::from_json(
        r#"{"type":"oauth","access_token":"a","refresh_token":"r","expires_at_unix_seconds":1781701200}"#,
    )
    .expect("parse oauth");
    assert_eq!(
        with_refresh,
        ProviderCredentialJson::Oauth {
            access_token: "a".to_string(),
            refresh_token: Some("r".to_string()),
            expires_at_unix_seconds: 1_781_701_200,
        }
    );
    let without_refresh = ProviderCredentialJson::from_json(
        r#"{"type":"oauth","access_token":"a","expires_at_unix_seconds":0}"#,
    )
    .expect("parse oauth without refresh");
    assert!(matches!(
        without_refresh,
        ProviderCredentialJson::Oauth {
            refresh_token: None,
            ..
        }
    ));
}

#[test]
fn submit_for_pr_response_carries_structured_gate_data() {
    let response = SubmitForPrResponse {
        accepted: false,
        message: "cargo test failed".to_string(),
        gates: vec![SubmitForPrGate {
            command_id: "pre-push:cargo-test".to_string(),
            argv: vec!["cargo".to_string(), "test".to_string()],
            cwd: "/workspace/temper".to_string(),
            exit_status: "failed".to_string(),
            exit_code: Some(101),
            stdout_tail: "running 1 test".to_string(),
            stderr_tail: "test failed".to_string(),
            timed_out: false,
            elapsed_ms: 1_234,
        }],
    };

    let json = serde_json::to_value(&response).expect("serialize submit response");
    assert_eq!(json["accepted"], false);
    assert_eq!(json["gates"][0]["command_id"], "pre-push:cargo-test");
    assert_eq!(json["gates"][0]["argv"][1], "test");
    assert_eq!(json["gates"][0]["cwd"], "/workspace/temper");
    assert_eq!(json["gates"][0]["exit_status"], "failed");
    assert_eq!(json["gates"][0]["exit_code"], 101);
    assert_eq!(json["gates"][0]["stdout_tail"], "running 1 test");
    assert_eq!(json["gates"][0]["stderr_tail"], "test failed");
    assert_eq!(json["gates"][0]["timed_out"], false);
    assert_eq!(json["gates"][0]["elapsed_ms"], 1_234);
    let round_trip: SubmitForPrResponse = serde_json::from_value(json).expect("round trip");
    assert_eq!(round_trip, response);
}

#[test]
fn workspace_result_omits_empty_optionals_on_the_wire() {
    let result = WorkspaceResult {
        summary: Some("did the thing".to_string()),
        ..Default::default()
    };
    let value = serde_json::to_value(&result).expect("serialize");
    assert_eq!(value["summary"], "did the thing");
    assert!(value.get("verdict").is_none());
    assert!(value.get("children").is_none());
}

#[test]
fn workspace_result_carries_engineer_pr_title_and_body() {
    let result = WorkspaceResult {
        title: Some("Implement durable handoff".to_string()),
        body: Some("# Implementation report\n\nDone.".to_string()),
        summary: Some("implemented handoff".to_string()),
        ..Default::default()
    };
    let value = serde_json::to_value(&result).expect("serialize");
    assert_eq!(value["title"], "Implement durable handoff");
    assert_eq!(value["body"], "# Implementation report\n\nDone.");
    assert_eq!(value["summary"], "implemented handoff");
    assert!(value.get("verdict").is_none());
}

#[test]
fn workspace_result_ignores_legacy_plan_field() {
    let parsed: WorkspaceResult = serde_json::from_str(
        r#"{"summary":"legacy head path","plan":{"phases":["old checklist"]}}"#,
    )
    .expect("legacy result with plan parses");
    assert_eq!(parsed.summary.as_deref(), Some("legacy head path"));
}

#[test]
fn workspace_context_correlation_key_is_required_and_round_trips() {
    let json = r#"{
        "repos": [{"id":"1","owner":"acme","name":"svc","default_branch":"main",
                   "dir":"svc","access":"writable","base_branch":"main",
                   "branch_hint":"smith/engineer/issue-7"}],
        "work_item": {"role":"engineer","queue":"code","kind":"issue","target":"Issue { number: 7 }","context":"{}"},
        "action": "open_pr",
        "correlation_key": "pr-for-code-7"
    }"#;
    let context: WorkspaceContext = serde_json::from_str(json).expect("parse");
    assert_eq!(context.action, "open_pr");
    assert_eq!(context.correlation_key, "pr-for-code-7");
    assert_eq!(context.allowed_verdicts, Vec::<String>::new());
    assert!(context.verdict_contracts.is_empty());
    assert!(context.source_metadata.is_empty());
    assert!(context.artifact_context.is_none());
    assert_eq!(context.checkout, None);
    let primary = context.primary().expect("primary repo present");
    assert_eq!(primary.dir, "svc");
    assert!(primary.is_writable());
    assert_eq!(primary.base_branch, "main");
}

#[test]
fn workspace_context_carries_multiple_repos_with_access() {
    let json = r#"{
        "repos": [
            {"id":"1","owner":"ai","name":"temper","default_branch":"main",
             "dir":"temper","access":"writable","base_branch":"main",
             "branch_hint":"agent/coord-for-code-42"},
            {"id":"2","owner":"ai","name":"skein","default_branch":"main",
             "dir":"skein","access":"read_only","base_branch":"main"}
        ],
        "work_item": {"role":"engineer","queue":"code","kind":"issue","target":"Issue { number: 42 }","context":"{}"},
        "action": "open_pr",
        "correlation_key": "coord-for-code-42"
    }"#;
    let context: WorkspaceContext = serde_json::from_str(json).expect("parse");
    assert_eq!(context.action, "open_pr");
    assert_eq!(context.repos.len(), 2);
    assert!(context.repos[0].is_writable());
    assert!(!context.repos[1].is_writable());
    assert_eq!(context.repos[1].branch_hint, None);
}

#[test]
fn artifact_context_embedding_fixture_preserves_work_item_context() {
    let json = include_str!("../fixtures/workspace-context-artifact-context.json");
    let raw: serde_json::Value = serde_json::from_str(json).expect("golden fixture is json");
    let context: WorkspaceContext = serde_json::from_str(json).expect("golden fixture parses");

    assert_eq!(context.artifact_context.as_ref().unwrap().version, 1);
    assert_eq!(
        context.work_item.context,
        raw["work_item"]["context"].as_str().unwrap()
    );
    assert_eq!(
        serde_json::to_value(&context.work_item).unwrap(),
        raw["work_item"],
        "the legacy singular work-item context shape must not change"
    );
}
