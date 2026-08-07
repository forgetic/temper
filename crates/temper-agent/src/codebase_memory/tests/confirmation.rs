use super::*;
use serde_json::json;
use temper_protocol_agent::{CodebaseMemoryIndex, CodebaseMemoryMode, WorkspaceContext};

fn provider_key(context: &WorkspaceContext, index: usize) -> String {
    super::super::scope::provider_key_for_repo(&context.repos[index])
}

#[test]
fn normalized_provider_identity_is_confirmed_and_used_for_graph_reads() {
    for index in [
        CodebaseMemoryIndex::Blocking,
        CodebaseMemoryIndex::Background,
    ] {
        let dir = fake_server_script();
        let workspace = tempfile::tempdir().expect("workspace");
        let log_path = workspace.path().join(format!("normalized-{index:?}.log"));
        let workspace_cwd = workspace.path().to_path_buf();
        let context = workspace_context(&workspace_cwd, &[("acme", "demo", "demo")]);
        let stable_key = provider_key(&context, 0);
        let expected_project = format!("normalized-{stable_key}");
        let setup_log_path = log_path.clone();

        temper_agent_io::block_on(async move {
            let toolset = build_codebase_memory_toolset(
                Some(&config(
                    &dir,
                    CodebaseMemoryMode::Required,
                    index,
                    "normalized",
                    &setup_log_path,
                    json!({}),
                )),
                "engineer",
                &context,
                &workspace_cwd,
            )
            .await
            .expect("normalized provider identity is confirmed");

            let search = toolset
                .into_tools()
                .into_iter()
                .find(|tool| tool.name() == "codebase_memory_search_code")
                .expect("search wrapper present");
            let output = search
                .execute("normalized", json!({"query": "identity"}), None)
                .await
                .expect("confirmed identity is ready for graph reads");
            assert!(!output.is_error);
        });

        let statuses = calls_named(&log_path, "index_status");
        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0]["arguments"]["project"], stable_key);
        assert_eq!(statuses[1]["arguments"]["project"], expected_project);
        let upserts = calls_named(&log_path, "index_repository");
        assert_eq!(upserts.len(), 1);
        assert_eq!(upserts[0]["arguments"]["name"], stable_key);
        let graph = calls_named(&log_path, "search_code");
        assert_eq!(graph.len(), 1);
        assert_eq!(graph[0]["arguments"]["project"], expected_project);
    }
}
