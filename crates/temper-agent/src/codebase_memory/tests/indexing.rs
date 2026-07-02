use super::*;
use serde_json::json;
use temper_protocol_agent::{CodebaseMemoryIndex, CodebaseMemoryMode};

#[test]
fn codebase_memory_discovers_root_path_projects_without_reindexing_and_defaults_actual_name() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("mcp.log");
    let context = workspace_context(
        workspace.path(),
        &[("acme", "app", "app"), ("acme", "lib", "lib")],
    );
    let app_path = workspace.path().join("app").canonicalize().unwrap();
    let lib_path = workspace.path().join("lib").canonicalize().unwrap();

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Blocking,
                "normal",
                &log_path,
                json!({"projects": [
                    {"name": "generated-app", "root_path": app_path},
                    {"name": "generated-lib", "rootPath": lib_path}
                ]}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("root_path/rootPath projects are discovered");
        assert!(calls_named(&log_path, "index_repository").is_empty());

        let tools = toolset.into_tools();
        let search = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("search wrapper present");
        search
            .execute("default", json!({"query": "app"}), None)
            .await
            .expect("default uses generated project name");
        search
            .execute("lib", json!({"query": "lib", "project": "acme/lib"}), None)
            .await
            .expect("alias uses generated project name");

        let search_calls = calls_named(&log_path, "search_code");
        assert_eq!(search_calls[0]["arguments"]["project"], "generated-app");
        assert_eq!(search_calls[1]["arguments"]["project"], "generated-lib");
    });
}

#[test]
fn codebase_memory_index_off_does_not_call_index_repository_and_marks_stale() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("mcp.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
    let repo_path = workspace.path().join("demo");

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Off,
                "normal",
                &log_path,
                json!({"projects": [{"id": "old-demo", "name": "acme/demo", "path": repo_path, "stale": true}]}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("build toolset with index off");
        let prompt_status = toolset.prompt_status().expect("prompt status");
        assert!(prompt_status.contains("index=off"));
        assert!(prompt_status.contains("stale according to codebase-memory project metadata"));
        assert!(calls_named(&log_path, "index_repository").is_empty());
    });
}

#[test]
fn codebase_memory_background_indexing_calls_only_prepared_repo_paths() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("mcp.log");
    let context = workspace_context(
        workspace.path(),
        &[("acme", "app", "app"), ("acme", "lib", "lib")],
    );
    let app_path = workspace.path().join("app").canonicalize().unwrap();
    let lib_path = workspace.path().join("lib").canonicalize().unwrap();

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Background,
                "normal",
                &log_path,
                json!({"projects": [
                    {"id": "evil", "name": "evil", "path": "/tmp/not-this-workspace"}
                ]}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("background indexing starts");
        let prompt_status = toolset.prompt_status().expect("prompt status");
        assert!(prompt_status.contains("background indexing may still be in progress"));

        let calls = wait_for_calls_named(&log_path, "index_repository", 2);
        assert!(
            calls
                .iter()
                .all(|call| call["arguments"].get("path").is_none())
        );
        let mut indexed_paths = calls
            .into_iter()
            .map(|call| call["arguments"]["repo_path"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        indexed_paths.sort();
        let mut expected = vec![
            app_path.display().to_string(),
            lib_path.display().to_string(),
        ];
        expected.sort();
        assert_eq!(indexed_paths, expected);
    });
}

#[test]
fn codebase_memory_blocking_indexing_success_and_timeout_modes() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("mcp.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
    let repo_path = workspace.path().join("demo").canonicalize().unwrap();

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Blocking,
                "normal",
                &log_path,
                json!({"projects": []}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("blocking indexing succeeds");
        assert!(
            toolset
                .prompt_status()
                .expect("prompt status")
                .contains("blocking indexing completed")
        );
        let calls = calls_named(&log_path, "index_repository");
        assert_eq!(calls.len(), 1);
        assert!(calls[0]["arguments"].get("path").is_none());
        assert_eq!(
            calls[0]["arguments"]["repo_path"],
            repo_path.display().to_string()
        );
    });

    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("timeout.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
    temper_agent_io::block_on(async move {
        let error = match build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Blocking,
                "index-hang",
                &log_path,
                json!({"projects": []}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        {
            Ok(_) => panic!("required blocking indexing timeout fails setup"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("timed out"));
    });

    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("auto-timeout.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Auto,
                CodebaseMemoryIndex::Blocking,
                "index-hang",
                &log_path,
                json!({"projects": []}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("auto blocking indexing timeout continues");
        assert_eq!(toolset.status(), &CodebaseMemoryToolsetStatus::Started);
        assert!(
            toolset
                .prompt_status()
                .expect("prompt status")
                .contains("continuing in auto mode")
        );
        let tools = toolset.into_tools();
        let search = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("search remains available after auto indexing timeout");
        let output = search
            .execute("after-timeout", json!({"query": "still works"}), None)
            .await
            .expect("main MCP client survives indexing timeout");
        assert!(output_text(&output).contains("still works"));
    });
}

#[test]
fn codebase_memory_blocking_index_rediscovers_actual_project_by_root_path() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("mcp.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Blocking,
                "index-rediscovers",
                &log_path,
                json!({"projects": []}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("blocking index rediscovery succeeds");
        assert!(
            toolset
                .prompt_status()
                .expect("prompt status")
                .contains("actual `generated-project`")
        );

        let tools = toolset.into_tools();
        let search = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("search wrapper present");
        search
            .execute("default", json!({"query": "needle"}), None)
            .await
            .expect("default uses rediscovered project name");

        assert_eq!(calls_named(&log_path, "index_repository").len(), 1);
        assert!(calls_named(&log_path, "list_projects").len() >= 2);
        let search_calls = calls_named(&log_path, "search_code");
        assert_eq!(search_calls[0]["arguments"]["project"], "generated-project");
    });
}
