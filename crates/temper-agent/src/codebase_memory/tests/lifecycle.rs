use super::*;
use serde_json::json;
use temper_protocol_agent::{CodebaseMemoryIndex, CodebaseMemoryMode, WorkspaceContext};

fn provider_key(context: &WorkspaceContext, index: usize) -> String {
    super::super::scope::provider_key_for_repo(&context.repos[index])
}

#[test]
fn cold_stable_projects_become_usable_and_warm_across_relocated_checkouts() {
    for index in [
        CodebaseMemoryIndex::Blocking,
        CodebaseMemoryIndex::Background,
    ] {
        let dir = fake_server_script();
        let first = tempfile::tempdir().expect("first workspace");
        let second = tempfile::tempdir().expect("relocated workspace");
        let log_path = first.path().join(format!("cold-warm-{index:?}.log"));
        let first_context = workspace_context(first.path(), &[("acme", "demo", "demo")]);
        let second_context = workspace_context(second.path(), &[("acme", "demo", "checkout")]);
        let stable_key = provider_key(&first_context, 0);
        let first_cwd = first.path().to_path_buf();
        let second_cwd = second.path().to_path_buf();
        let setup_log_path = log_path.clone();
        assert_eq!(stable_key, provider_key(&second_context, 0));

        temper_agent_io::block_on(async move {
            let first_toolset = build_codebase_memory_toolset(
                Some(&config(
                    &dir,
                    CodebaseMemoryMode::Required,
                    index,
                    "cold-warm",
                    &setup_log_path,
                    json!({}),
                )),
                "engineer",
                &first_context,
                &first_cwd,
            )
            .await
            .expect("cold stable upsert starts");
            let first_search = first_toolset
                .into_tools()
                .into_iter()
                .find(|tool| tool.name() == "codebase_memory_search_code")
                .expect("cold search wrapper");
            let cold = first_search
                .execute("cold", json!({"query": "stable"}), None)
                .await
                .expect("cold project is usable after stable upsert");
            assert!(!cold.is_error);

            let warm_toolset = build_codebase_memory_toolset(
                Some(&config(
                    &dir,
                    CodebaseMemoryMode::Required,
                    index,
                    "cold-warm",
                    &setup_log_path,
                    json!({}),
                )),
                "engineer",
                &second_context,
                &second_cwd,
            )
            .await
            .expect("relocated checkout reuses the stable project");
            let warm_search = warm_toolset
                .into_tools()
                .into_iter()
                .find(|tool| tool.name() == "codebase_memory_search_code")
                .expect("warm search wrapper");
            let warm = warm_search
                .execute("warm", json!({"query": "stable"}), None)
                .await
                .expect("warm project remains usable");
            assert!(!warm.is_error);
        });

        let upserts = calls_named(&log_path, "index_repository");
        assert_eq!(
            upserts.len(),
            1,
            "{index:?} must not recreate a warm project"
        );
        assert_eq!(upserts[0]["arguments"]["name"], stable_key);
        assert_eq!(calls_named(&log_path, "index_status").len(), 2);
        let searches = calls_named(&log_path, "search_code");
        assert_eq!(searches.len(), 2);
        assert!(
            searches
                .iter()
                .all(|call| call["arguments"]["project"] == stable_key)
        );
        let state = provider_state(&log_path);
        assert_eq!(state["projects"].as_object().unwrap().len(), 1);
        assert_eq!(state["counters"]["project_creations"], 1);
        assert_eq!(state["counters"]["index_repository"], 1);
    }
}

#[test]
fn unconfirmed_or_failed_stable_upserts_are_redacted_and_not_retried() {
    for (index, server_mode) in [
        (CodebaseMemoryIndex::Blocking, "index-error-secret"),
        (CodebaseMemoryIndex::Background, "index-malformed"),
        (CodebaseMemoryIndex::Background, "index-wrong-project"),
    ] {
        let dir = fake_server_script();
        let workspace = tempfile::tempdir().expect("workspace");
        let log_path = workspace
            .path()
            .join(format!("{server_mode}-{index:?}.log"));
        let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
        let workspace_cwd = workspace.path().to_path_buf();
        let setup_log_path = log_path.clone();

        temper_agent_io::block_on(async move {
            let toolset = build_codebase_memory_toolset(
                Some(&config(
                    &dir,
                    CodebaseMemoryMode::Auto,
                    index,
                    server_mode,
                    &setup_log_path,
                    json!({}),
                )),
                "engineer",
                &context,
                &workspace_cwd,
            )
            .await
            .expect("auto mode retains conventional fallback after failed upsert");
            assert!(!toolset.prompt_status().unwrap().contains("SECRET"));
            let tools = toolset.into_tools();
            let search = tools
                .iter()
                .find(|tool| tool.name() == "codebase_memory_search_code")
                .expect("search wrapper");
            let architecture = tools
                .iter()
                .find(|tool| tool.name() == "codebase_memory_get_architecture")
                .expect("architecture wrapper");

            let failed = search
                .execute("failed-upsert", json!({"query": "stable"}), None)
                .await
                .expect("failed stable upsert is a typed tool result");
            assert_eq!(
                failed.details.as_ref().unwrap()[SAFE_TOOL_FAILURE_DETAIL_KEY]["category"],
                "index_failure"
            );
            assert!(output_text(&failed).contains("conventional discovery"));
            assert!(!output_text(&failed).contains("SECRET"));

            let retry = architecture
                .execute("no-immediate-retry", json!({}), None)
                .await
                .expect("open circuit prevents immediate retry");
            assert_eq!(
                retry.details.as_ref().unwrap()[SAFE_TOOL_FAILURE_DETAIL_KEY]["category"],
                "circuit_open"
            );
        });

        assert_eq!(calls_named(&log_path, "index_repository").len(), 1);
        assert!(calls_named(&log_path, "search_code").is_empty());
        assert!(calls_named(&log_path, "get_architecture").is_empty());
    }
}
