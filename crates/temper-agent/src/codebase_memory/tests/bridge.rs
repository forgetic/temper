use super::test_support::*;
use super::*;
use serde_json::json;
use temper_protocol_activity::{
    GraphCorrelationTargetKindV1, GraphCorrelationToolV1, GraphCorrelationV1,
};
use temper_protocol_agent::{CodebaseMemoryIndex, CodebaseMemoryMode};
use tongs::tools::ToolEffects;

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
            .execute(
                "call-1",
                json!({ "query": "needle", "pattern": "ToolFinishedV1" }),
                None,
            )
            .await
            .expect("execute wrapped MCP tool");
        let correlation: GraphCorrelationV1 = serde_json::from_value(
            output
                .details
                .as_ref()
                .and_then(|details| details.get(SAFE_GRAPH_CORRELATION_DETAIL_KEY))
                .cloned()
                .expect("wrapper emits a closed correlation detail"),
        )
        .expect("correlation detail is typed");
        assert_eq!(
            correlation,
            GraphCorrelationV1::new(
                GraphCorrelationToolV1::SearchCode,
                GraphCorrelationTargetKindV1::Pattern,
                "ToolFinishedV1",
            )
            .expect("complete declared pattern")
        );
        assert!(
            !serde_json::to_string(&output.details)
                .expect("details serialize")
                .contains("ToolFinishedV1")
        );
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
