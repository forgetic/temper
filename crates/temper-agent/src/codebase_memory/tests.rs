mod indexing;
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

        let unknown = match search
            .execute(
                "unknown",
                json!({ "query": "Widget", "project": "other/repo" }),
                None,
            )
            .await
        {
            Ok(_) => panic!("unknown aliases are rejected"),
            Err(error) => error,
        };
        assert!(
            unknown
                .to_string()
                .contains("unknown codebase-memory project/repo alias")
        );

        let provider_identity = match search
            .execute(
                "provider-key",
                json!({ "query": "Widget", "project": provider_key }),
                None,
            )
            .await
        {
            Ok(_) => panic!("provider identities are not model-facing aliases"),
            Err(error) => error,
        };
        assert!(
            provider_identity
                .to_string()
                .contains("unknown codebase-memory project/repo alias")
        );

        let path_alias = match search
            .execute(
                "path-alias",
                json!({ "query": "Widget", "repo": "/tmp/demo" }),
                None,
            )
            .await
        {
            Ok(_) => panic!("filesystem paths are not project aliases"),
            Err(error) => error,
        };
        assert!(
            path_alias
                .to_string()
                .contains("filesystem paths are not accepted")
        );

        let unsafe_path = match search
            .execute(
                "unsafe-path",
                json!({ "query": "Widget", "path": "/etc/passwd" }),
                None,
            )
            .await
        {
            Ok(_) => panic!("absolute paths are rejected"),
            Err(error) => error,
        };
        assert!(unsafe_path.to_string().contains("repository-relative path"));
        let nested_unsafe_path = match search
            .execute(
                "nested-unsafe-path",
                json!({ "query": "Widget", "filters": {"path": "../secret"} }),
                None,
            )
            .await
        {
            Ok(_) => panic!("nested parent paths are rejected"),
            Err(error) => error,
        };
        assert!(
            nested_unsafe_path
                .to_string()
                .contains("selected workspace repository")
        );
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
