mod health;
mod indexing;
mod lifecycle;
#[path = "test_support.rs"]
mod test_support;
use super::*;
use serde_json::json;
use std::fs;
use temper_protocol_agent::{CodebaseMemoryIndex, CodebaseMemoryMode};
use test_support::*;
use tongs::tools::ToolEffects;

#[test]
fn model_visible_mcp_calls_are_clamped_to_the_generic_tool_deadline() {
    assert_eq!(
        effective_mcp_call_timeout(Duration::from_secs(900), Duration::from_secs(30)),
        Duration::from_secs(30)
    );
    assert_eq!(
        effective_mcp_call_timeout(Duration::from_secs(5), Duration::from_secs(30)),
        Duration::from_secs(5)
    );
}

#[test]
fn codebase_memory_failures_are_classified_without_retaining_source_text() {
    let cases = [
        (
            McpError::Spawn {
                command: "command --token=SECRET".to_string(),
                message: "stderr SECRET".to_string(),
            },
            ToolFailureCategory::ConfigurationStartup,
        ),
        (
            McpError::Io {
                operation: "read response",
                message: "transport SECRET".to_string(),
            },
            ToolFailureCategory::Transport,
        ),
        (
            McpError::Timeout {
                method: "tools/call".to_string(),
                timeout: Duration::from_secs(1),
            },
            ToolFailureCategory::Timeout,
        ),
        (
            McpError::ProcessExited {
                method: "tools/call".to_string(),
                status: Some("SECRET".to_string()),
            },
            ToolFailureCategory::ProcessExit,
        ),
        (
            McpError::Protocol("provider cache SECRET".to_string()),
            ToolFailureCategory::ProviderProtocol,
        ),
        (
            McpError::Json {
                operation: "encode request",
                message: "model input SECRET".to_string(),
            },
            ToolFailureCategory::InvalidModelInput,
        ),
        (
            McpError::Json {
                operation: "decode response",
                message: "provider SECRET".to_string(),
            },
            ToolFailureCategory::ProviderProtocol,
        ),
        (
            McpError::Rpc {
                method: "tools/call".to_string(),
                message: r#"{"code":-32602,"message":"Invalid params SECRET"}"#.to_string(),
            },
            ToolFailureCategory::InvalidModelInput,
        ),
        (
            McpError::ProtocolOverflow {
                direction: "outbound",
                resource: "record bytes",
                limit: 10,
                observed: 11,
            },
            ToolFailureCategory::InvalidModelInput,
        ),
    ];
    for (error, expected) in cases {
        let output = codebase_memory_failure_output(classify_mcp_error(&error));
        assert!(output.is_error);
        assert_eq!(
            output
                .details
                .as_ref()
                .and_then(|details| { details[SAFE_TOOL_FAILURE_DETAIL_KEY]["category"].as_str() }),
            Some(expected.as_str())
        );
        let rendered = format!(
            "{} {}",
            serde_json::to_string(&output.details).unwrap(),
            output_text(&output)
        );
        assert!(!rendered.contains("SECRET"));
        assert!(!rendered.contains("command --token"));
        assert!(!rendered.contains("provider cache"));
    }

    assert_eq!(
        classify_input_failure("background indexing failed: repository SECRET"),
        ToolFailureCategory::IndexFailure
    );
    assert_eq!(
        classify_input_failure(
            "project is not ready: background indexing is still in progress after 0.250s"
        ),
        ToolFailureCategory::Timeout
    );
    assert_eq!(
        classify_input_failure("project is not ready: repository SECRET"),
        ToolFailureCategory::ProjectNotReady
    );
    assert_eq!(
        classify_provider_failure("invalid argument contains repository SECRET"),
        ToolFailureCategory::InvalidModelInput
    );
    assert_eq!(
        classify_provider_failure("project not found"),
        ToolFailureCategory::ProjectNotReady
    );
    assert_eq!(
        classify_provider_failure("no matches"),
        ToolFailureCategory::ProviderProtocol
    );
}

#[test]
fn codebase_memory_bridge_wraps_allowed_tool_and_filters_destructive_tools() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("mcp.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
    let provider_key = scope::provider_key_for_repo(&context.repos[0]);
    let repo_path = workspace.path().join("demo");
    let projects =
        json!({"projects": [{"id": "project-demo", "name": "acme/demo", "path": repo_path}]});

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Off,
                "normal",
                &log_path,
                projects,
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("build required codebase-memory toolset");
        assert_eq!(toolset.status(), &CodebaseMemoryToolsetStatus::Started);
        let names = toolset.registered_tool_names().to_vec();
        assert!(names.contains(&"codebase_memory_search_code".to_string()));
        assert!(names.contains(&"codebase_memory_get_architecture".to_string()));
        for forbidden in [
            "codebase_memory_delete_project",
            "codebase_memory_manage_adr",
            "codebase_memory_ingest_traces",
            "codebase_memory_query_graph",
            "codebase_memory_index_repository",
        ] {
            assert!(
                !names.contains(&forbidden.to_string()),
                "{forbidden} must not be registered"
            );
        }

        let tools = toolset.into_tools();
        let search = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("search wrapper present");
        assert_eq!(search.effects(), ToolEffects::read());
        let output = search
            .execute("call-1", json!({ "query": "needle" }), None)
            .await
            .expect("execute wrapped MCP tool");
        let text = output_text(&output);
        assert!(!output.is_error);
        assert!(text.contains("search_code result"));
        assert!(text.contains("needle"));
        assert!(text.contains(&provider_key));
        assert!(text.contains("output truncated"));
        assert!(text.len() <= MAX_CODEBASE_MEMORY_OUTPUT_BYTES);
    });
}

#[test]
fn codebase_memory_workspace_aliases_default_primary_and_filter_project_list() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("mcp.log");
    let context = workspace_context(
        workspace.path(),
        &[("acme", "app", "app"), ("acme", "lib", "lib")],
    );
    let app_key = scope::provider_key_for_repo(&context.repos[0]);
    let lib_key = scope::provider_key_for_repo(&context.repos[1]);
    let projects = json!({"projects": [
        {"id": "cbm-app", "name": "app-index", "path": workspace.path().join("app")},
        {"id": "cbm-lib", "name": "lib-index", "path": workspace.path().join("lib")},
        {"id": "evil", "name": "evil", "path": "/tmp/not-this-workspace"}
    ]});

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Off,
                "normal",
                &log_path,
                projects,
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("build toolset");
        let tools = toolset.into_tools();
        let search = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("search wrapper present");
        search
            .execute("default", json!({ "query": "Widget" }), None)
            .await
            .expect("default project injected");
        search
            .execute(
                "alias",
                json!({ "query": "Helper", "project": "acme/lib" }),
                None,
            )
            .await
            .expect("alias translated");
        search
            .execute(
                "repo-alias",
                json!({ "query": "Helper", "repo": "acme/lib" }),
                None,
            )
            .await
            .expect("repo alias translated onto the MCP project field");

        let search_calls = calls_named(&log_path, "search_code");
        assert_eq!(search_calls.len(), 3);
        assert_eq!(search_calls[0]["arguments"]["project"], app_key);
        assert_eq!(search_calls[1]["arguments"]["project"], lib_key);
        assert_eq!(search_calls[2]["arguments"]["project"], lib_key);
        assert!(search_calls[2]["arguments"].get("repo").is_none());

        let list = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_list_projects")
            .expect("list wrapper present");
        let output = list
            .execute("list", json!({}), None)
            .await
            .expect("workspace-scoped list works");
        let text = output_text(&output);
        assert!(text.contains("acme/app"));
        assert!(text.contains("acme/lib"));
        assert!(
            !text.contains("evil"),
            "list_projects must not leak arbitrary MCP projects: {text}"
        );
    });
}

#[test]
fn codebase_memory_normalizes_aliases_for_advertised_repo_schemas() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("repo-schema.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
    let provider_key = scope::provider_key_for_repo(&context.repos[0]);

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Off,
                "repo-schema",
                &log_path,
                json!({}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("repo-schema provider starts");
        let search = toolset
            .into_tools()
            .into_iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("search wrapper present");
        let parameters = search.parameters();
        assert_eq!(
            parameters["properties"]["project"]["enum"],
            parameters["properties"]["repo"]["enum"]
        );
        assert_eq!(parameters["required"], json!(["query"]));

        let output = search
            .execute(
                "project-alias",
                json!({"query": "needle", "project": "acme/demo"}),
                None,
            )
            .await
            .expect("project alias is normalized onto provider repo input");
        assert!(!output.is_error);
        let calls = calls_named(&log_path, "search_code");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["arguments"]["repo"], provider_key);
        assert!(calls[0]["arguments"].get("project").is_none());
    });
}

#[test]
fn codebase_memory_rejects_unknown_aliases_and_unsafe_paths() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("mcp.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
    let provider_key = scope::provider_key_for_repo(&context.repos[0]);
    let projects = json!({"projects": [{"id": "cbm-demo", "name": "demo-index", "path": workspace.path().join("demo")}]});

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Off,
                "normal",
                &log_path,
                projects,
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("build toolset");
        let tools = toolset.into_tools();
        let search = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("search wrapper present");

        for (call_id, input) in [
            (
                "unknown",
                json!({ "query": "Widget", "project": "other/repo" }),
            ),
            (
                "provider-key",
                json!({ "query": "Widget", "project": provider_key }),
            ),
            (
                "path-alias",
                json!({ "query": "Widget", "repo": "/tmp/demo" }),
            ),
            (
                "unsafe-path",
                json!({ "query": "Widget", "path": "/etc/passwd" }),
            ),
            (
                "nested-unsafe-path",
                json!({ "query": "Widget", "filters": {"path": "../secret"} }),
            ),
        ] {
            let output = search
                .execute(call_id, input, None)
                .await
                .expect("invalid input becomes a model-visible tool failure");
            assert!(output.is_error);
            assert!(output_text(&output).contains("request input was invalid"));
            assert_eq!(
                output.details.as_ref().and_then(|details| {
                    details[SAFE_TOOL_FAILURE_DETAIL_KEY]["category"].as_str()
                }),
                Some("invalid_model_input")
            );
            let rendered = serde_json::to_string(&output.details).unwrap();
            assert!(!rendered.contains("/etc/passwd"));
            assert!(!rendered.contains("../secret"));
            assert!(!rendered.contains("other/repo"));
        }

        let output = search
            .execute(
                "valid-after-local-errors",
                json!({"query": "still healthy"}),
                None,
            )
            .await
            .expect("query-local failures leave the shared client healthy");
        assert!(!output.is_error);
        assert_eq!(calls_named(&log_path, "search_code").len(), 1);
    });
}

#[test]
fn codebase_memory_workspace_path_safety_and_single_repo_cwd_checkout() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("mcp.log");
    let mut unsafe_context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
    unsafe_context.repos[0].dir = "../outside".to_string();

    temper_agent_io::block_on(async move {
        let error = match build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Off,
                "normal",
                &log_path,
                json!({"projects": []}),
            )),
            "engineer",
            &unsafe_context,
            workspace.path(),
        )
        .await
        {
            Ok(_) => panic!("parent-dir repo roots are rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("unsafe dir"));
    });

    let dir = fake_server_script();
    let repo_cwd = tempfile::tempdir().expect("single repo cwd");
    fs::create_dir_all(repo_cwd.path().join(".git")).expect("fake git dir");
    let log_path = repo_cwd.path().join("single.log");
    let mut context = workspace_context(repo_cwd.path(), &[("acme", "demo", ".")]);
    context.repos[0].dir = "demo".to_string();
    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Off,
                "normal",
                &log_path,
                json!({"projects": []}),
            )),
            "engineer",
            &context,
            repo_cwd.path(),
        )
        .await
        .expect("single-repo cwd checkout resolves safely even when repo.dir names the checkout");
        assert_eq!(toolset.status(), &CodebaseMemoryToolsetStatus::Started);
    });
}

#[test]
fn codebase_memory_bridge_auto_vs_required_startup_failures() {
    let auto = bad_command_config(CodebaseMemoryMode::Auto);
    let required = bad_command_config(CodebaseMemoryMode::Required);
    let workspace = tempfile::tempdir().expect("workspace");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);

    temper_agent_io::block_on(async move {
        let auto_toolset =
            build_codebase_memory_toolset(Some(&auto), "engineer", &context, workspace.path())
                .await
                .expect("auto mode suppresses startup failure");
        assert!(matches!(
            auto_toolset.status(),
            CodebaseMemoryToolsetStatus::AutoUnavailable { reason }
                if reason.contains("spawn MCP command")
        ));
        assert!(auto_toolset.registered_tool_names().is_empty());

        let required_error = match build_codebase_memory_toolset(
            Some(&required),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        {
            Ok(_) => panic!("required mode hard-fails startup failure"),
            Err(error) => error,
        };
        assert!(
            required_error
                .to_string()
                .contains("required codebase-memory MCP startup failed")
        );
    });
}

#[test]
fn codebase_memory_bridge_auto_timeout_is_best_effort_required_timeout_fails() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("startup-timeout.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
    temper_agent_io::block_on(async move {
        let auto = config(
            &dir,
            CodebaseMemoryMode::Auto,
            CodebaseMemoryIndex::Off,
            "hang",
            &log_path,
            json!({"projects": []}),
        );
        let auto_toolset =
            build_codebase_memory_toolset(Some(&auto), "engineer", &context, workspace.path())
                .await
                .expect("auto mode suppresses timeout");
        assert!(matches!(
            auto_toolset.status(),
            CodebaseMemoryToolsetStatus::AutoUnavailable { reason }
                if reason.contains("timed out")
        ));

        let required = config(
            &dir,
            CodebaseMemoryMode::Required,
            CodebaseMemoryIndex::Off,
            "hang",
            &log_path,
            json!({"projects": []}),
        );
        let error = match build_codebase_memory_toolset(
            Some(&required),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        {
            Ok(_) => panic!("required mode fails timeout"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("timed out"));
    });
}
