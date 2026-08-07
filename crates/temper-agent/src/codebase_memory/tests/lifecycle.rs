use super::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use temper_protocol_agent::{CodebaseMemoryIndex, CodebaseMemoryMode, WorkspaceContext};

fn provider_key(context: &WorkspaceContext, index: usize) -> String {
    super::super::scope::provider_key_for_repo(&context.repos[index])
}

#[test]
fn fresh_stable_project_rebinds_to_relocated_checkout_before_serving_source() {
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
        let first_root = first
            .path()
            .join("demo")
            .canonicalize()
            .expect("canonical first checkout root");
        let second_root = second
            .path()
            .join("checkout")
            .canonicalize()
            .expect("canonical second checkout root");
        let first_source = "pub const CHECKOUT: &str = \"first\";\n";
        let second_source = "pub const CHECKOUT: &str = \"second\";\n";
        std::fs::create_dir_all(first.path().join("demo/src")).expect("create first source dir");
        std::fs::write(first.path().join("demo/src/lib.rs"), first_source)
            .expect("write first source");
        std::fs::create_dir_all(second.path().join("checkout/src"))
            .expect("create second source dir");
        std::fs::write(second.path().join("checkout/src/lib.rs"), second_source)
            .expect("write second source");
        let stable_key = provider_key(&first_context, 0);
        let first_cwd = first.path().to_path_buf();
        let second_cwd = second.path().to_path_buf();
        let second_source_path = second
            .path()
            .join("checkout/src/lib.rs")
            .canonicalize()
            .expect("canonical second source path");
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
            .expect("fresh logical project is rebound to the relocated checkout");
            let warm_tools = warm_toolset.into_tools();
            let warm_search = warm_tools
                .iter()
                .find(|tool| tool.name() == "codebase_memory_search_code")
                .expect("warm search wrapper");
            let warm = warm_search
                .execute("warm", json!({"query": "stable"}), None)
                .await
                .expect("warm project remains usable after rebind");
            assert!(!warm.is_error);

            let snippet = warm_tools
                .iter()
                .find(|tool| tool.name() == "codebase_memory_get_code_snippet")
                .expect("snippet wrapper");
            let snippet = snippet
                .execute("snippet", json!({"path": "src/lib.rs"}), None)
                .await
                .expect("rebound project serves source");
            assert!(!snippet.is_error);
            let payload: Value = serde_json::from_str(&output_text(&snippet))
                .expect("fixture snippet response is JSON");
            assert_eq!(payload["source"], second_source);
            assert_eq!(
                payload["file_path"],
                second_source_path.display().to_string(),
                "snippet source must come from the second live checkout"
            );
        });

        let upserts = calls_named(&log_path, "index_repository");
        assert_eq!(
            upserts.len(),
            2,
            "{index:?} must rebind the fresh stable project for each checkout"
        );
        assert!(
            upserts
                .iter()
                .all(|upsert| upsert["arguments"]["name"] == stable_key)
        );
        let roots = upserts
            .iter()
            .map(|upsert| {
                upsert["arguments"]["repo_path"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(roots.len(), 2, "each checkout root must be rebound");
        assert_eq!(
            roots,
            BTreeSet::from([
                first_root.display().to_string(),
                second_root.display().to_string(),
            ]),
            "each upsert must request its canonical prepared checkout root"
        );
        assert_eq!(calls_named(&log_path, "index_status").len(), 4);
        assert!(calls_named(&log_path, "list_projects").is_empty());
        let searches = calls_named(&log_path, "search_code");
        assert_eq!(searches.len(), 2);
        assert!(
            searches
                .iter()
                .all(|call| call["arguments"]["project"] == stable_key)
        );
        let snippets = calls_named(&log_path, "get_code_snippet");
        assert_eq!(snippets.len(), 1);
        assert_eq!(snippets[0]["arguments"]["project"], stable_key);
        let state = provider_state(&log_path);
        assert_eq!(state["projects"].as_object().unwrap().len(), 1);
        assert_eq!(state["counters"]["project_creations"], 1);
        assert_eq!(state["counters"]["index_repository"], 2);
        assert_eq!(
            state["projects"][&stable_key]["repo_path"],
            second_root.display().to_string(),
            "the one retained provider project must be bound to the live second checkout"
        );
    }
}

#[test]
fn unconfirmed_or_failed_stable_upserts_are_redacted_and_not_retried() {
    for (index, server_mode) in [
        (CodebaseMemoryIndex::Blocking, "index-error-secret"),
        (CodebaseMemoryIndex::Background, "index-malformed"),
        (CodebaseMemoryIndex::Background, "index-wrong-project"),
        (CodebaseMemoryIndex::Blocking, "index-missing-root"),
        (CodebaseMemoryIndex::Blocking, "index-malformed-root"),
        (CodebaseMemoryIndex::Background, "index-wrong-root"),
        (CodebaseMemoryIndex::Background, "index-unconfirmed-root"),
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

#[test]
fn confirmation_identity_and_root_failures_are_bounded_in_all_index_modes() {
    for server_mode in [
        "confirmation-missing-identity",
        "confirmation-malformed-identity",
        "confirmation-mismatched-identity",
        "confirmation-path-keyed-identity",
        "index-missing-root",
        "index-malformed-root",
        "index-wrong-root",
    ] {
        for mode in [CodebaseMemoryMode::Auto, CodebaseMemoryMode::Required] {
            for index in [
                CodebaseMemoryIndex::Blocking,
                CodebaseMemoryIndex::Background,
            ] {
                let dir = fake_server_script();
                let workspace = tempfile::tempdir().expect("workspace");
                let log_path = workspace
                    .path()
                    .join(format!("{server_mode}-{mode:?}-{index:?}.log"));
                let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
                let workspace_cwd = workspace.path().to_path_buf();
                let workspace_path = workspace_cwd.display().to_string();
                let setup_log_path = log_path.clone();

                temper_agent_io::block_on(async move {
                    let result = build_codebase_memory_toolset(
                        Some(&config(
                            &dir,
                            mode,
                            index,
                            server_mode,
                            &setup_log_path,
                            json!({}),
                        )),
                        "engineer",
                        &context,
                        &workspace_cwd,
                    )
                    .await;
                    if mode == CodebaseMemoryMode::Required
                        && index == CodebaseMemoryIndex::Blocking
                    {
                        let error = match result {
                            Ok(_) => panic!("required blocking mode rejects confirmation"),
                            Err(error) => error,
                        };
                        let rendered = error.to_string();
                        assert!(rendered.contains("stable codebase-memory index upsert"));
                        assert!(rendered.contains("was not confirmed"));
                        assert!(
                            !rendered.contains(&workspace_path),
                            "provider paths must not be exposed in required-mode diagnostics"
                        );
                    } else {
                        let toolset = result.expect("nonblocking confirmation failures are typed");
                        let search = toolset
                            .into_tools()
                            .into_iter()
                            .find(|tool| tool.name() == "codebase_memory_search_code")
                            .expect("search wrapper");
                        let failed = search
                            .execute("failed-confirmation", json!({"query": "stable"}), None)
                            .await
                            .expect("failed confirmation is a typed tool result");
                        assert!(failed.is_error);
                        assert_eq!(
                            failed.details.as_ref().unwrap()[SAFE_TOOL_FAILURE_DETAIL_KEY]["category"],
                            "index_failure"
                        );
                    }
                });

                assert_eq!(calls_named(&log_path, "index_repository").len(), 1);
                assert_eq!(calls_named(&log_path, "index_status").len(), 2);
                assert!(calls_named(&log_path, "search_code").is_empty());
                assert!(calls_named(&log_path, "list_projects").is_empty());
            }
        }
    }
}
