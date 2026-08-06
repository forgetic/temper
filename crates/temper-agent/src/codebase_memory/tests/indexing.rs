use super::*;
use serde_json::json;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};
use temper_protocol_agent::{CodebaseMemoryIndex, CodebaseMemoryMode, WorkspaceContext};

fn provider_key(context: &WorkspaceContext, index: usize) -> String {
    super::super::scope::provider_key_for_repo(&context.repos[index])
}

#[test]
fn stable_provider_identity_ignores_checkout_path_and_separates_repositories() {
    let first = tempfile::tempdir().expect("first workspace");
    let second = tempfile::tempdir().expect("second workspace");
    let first_context = workspace_context(first.path(), &[("acme", "app", "app")]);
    let second_context = workspace_context(second.path(), &[("acme", "app", "different-dir")]);

    assert_eq!(
        provider_key(&first_context, 0),
        provider_key(&second_context, 0)
    );
    assert!(provider_key(&first_context, 0).starts_with("temper-v1-"));
    assert!(!provider_key(&first_context, 0).contains(&first.path().display().to_string()));

    let multi = workspace_context(
        first.path(),
        &[("acme", "app", "app"), ("acme", "lib", "lib")],
    );
    assert_ne!(provider_key(&multi, 0), provider_key(&multi, 1));
}

#[test]
fn targeted_discovery_reuses_fresh_projects_without_global_inventory() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("mcp.log");
    let context = workspace_context(
        workspace.path(),
        &[("acme", "app", "app"), ("acme", "lib", "lib")],
    );
    let app_key = provider_key(&context, 0);
    let lib_key = provider_key(&context, 1);

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Blocking,
                "global-list-hang",
                &log_path,
                json!({}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("fresh projects are reused through targeted lookup");

        assert_eq!(calls_named(&log_path, "index_status").len(), 2);
        assert!(calls_named(&log_path, "list_projects").is_empty());
        assert!(calls_named(&log_path, "index_repository").is_empty());

        let tools = toolset.into_tools();
        let search = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("search wrapper present");
        search
            .execute("app", json!({"query": "app"}), None)
            .await
            .expect("primary provider key injected");
        search
            .execute("lib", json!({"query": "lib", "project": "acme/lib"}), None)
            .await
            .expect("secondary alias translated");
        let calls = calls_named(&log_path, "search_code");
        assert_eq!(calls[0]["arguments"]["project"], app_key);
        assert_eq!(calls[1]["arguments"]["project"], lib_key);
    });
}

#[test]
fn index_off_preserves_confirmed_missing_state_without_indexing() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("mcp.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Off,
                "missing",
                &log_path,
                json!({}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("index off still performs safe targeted discovery");
        let prompt = toolset.prompt_status().expect("prompt status");
        assert!(prompt.contains("Index setting: `off`"));
        assert!(prompt.contains("missing from codebase-memory index"));
        assert!(calls_named(&log_path, "index_repository").is_empty());
    });
}

#[test]
fn confirmed_missing_projects_use_stable_blocking_upsert_and_repeated_roots_converge() {
    let dir = fake_server_script();
    let first = tempfile::tempdir().expect("first workspace");
    let second = tempfile::tempdir().expect("second workspace");
    let log_path = first.path().join("mcp.log");
    let first_context = workspace_context(first.path(), &[("acme", "demo", "demo")]);
    let second_context = workspace_context(second.path(), &[("acme", "demo", "checkout")]);
    let stable_key = provider_key(&first_context, 0);
    assert_eq!(stable_key, provider_key(&second_context, 0));

    let first_config = config(
        &dir,
        CodebaseMemoryMode::Required,
        CodebaseMemoryIndex::Blocking,
        "missing",
        &log_path,
        json!({}),
    );
    let first_cwd = first.path().to_path_buf();
    let first_run_context = first_context.clone();
    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&first_config),
            "engineer",
            &first_run_context,
            &first_cwd,
        )
        .await
        .expect("first blocking stable upsert succeeds");
        assert!(
            toolset
                .prompt_status()
                .expect("prompt status")
                .contains("blocking indexing completed")
        );
    });

    let second_config = config(
        &dir,
        CodebaseMemoryMode::Required,
        CodebaseMemoryIndex::Blocking,
        "missing",
        &log_path,
        json!({}),
    );
    let second_cwd = second.path().to_path_buf();
    let second_run_context = second_context.clone();
    temper_agent_io::block_on(async move {
        build_codebase_memory_toolset(
            Some(&second_config),
            "engineer",
            &second_run_context,
            &second_cwd,
        )
        .await
        .expect("second blocking stable upsert succeeds");
    });

    let calls = calls_named(&log_path, "index_repository");
    assert_eq!(calls.len(), 2);
    assert!(
        calls
            .iter()
            .all(|call| call["arguments"]["name"] == stable_key)
    );
    let roots = calls
        .iter()
        .map(|call| call["arguments"]["repo_path"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        roots.len(),
        2,
        "checkout paths differ while the upsert key remains stable"
    );
    assert!(calls_named(&log_path, "list_projects").is_empty());
}

#[test]
fn duplicate_provider_identity_is_indexed_once_per_preparation_pass() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("duplicate.log");
    let mut context = workspace_context(
        workspace.path(),
        &[("acme", "demo", "first"), ("acme", "demo", "second")],
    );
    context.repos[1].id = context.repos[0].id.clone();
    assert_eq!(provider_key(&context, 0), provider_key(&context, 1));

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Blocking,
                "missing",
                &log_path,
                json!({}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("duplicate stable identity converges through one upsert");

        assert_eq!(calls_named(&log_path, "index_status").len(), 2);
        assert_eq!(calls_named(&log_path, "index_repository").len(), 1);
        assert!(
            toolset
                .prompt_status()
                .expect("prompt status")
                .contains("duplicate stable index request was suppressed")
        );
    });
}

#[test]
fn stale_multi_repo_background_upserts_keep_paths_and_provider_keys_isolated() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("mcp.log");
    let context = workspace_context(
        workspace.path(),
        &[("acme", "app", "app"), ("acme", "lib", "lib")],
    );
    let expected_names = [provider_key(&context, 0), provider_key(&context, 1)]
        .into_iter()
        .collect::<BTreeSet<_>>();
    let expected_paths = context
        .repos
        .iter()
        .map(|repo| {
            workspace
                .path()
                .join(&repo.dir)
                .canonicalize()
                .unwrap()
                .display()
                .to_string()
        })
        .collect::<BTreeSet<_>>();

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Background,
                "stale",
                &log_path,
                json!({}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("background stable upserts start");
        assert!(
            toolset
                .prompt_status()
                .expect("prompt status")
                .contains("background indexing may still be in progress")
        );

        let calls = wait_for_calls_named(&log_path, "index_repository", 2);
        let names = calls
            .iter()
            .map(|call| call["arguments"]["name"].as_str().unwrap().to_string())
            .collect::<BTreeSet<_>>();
        let paths = calls
            .iter()
            .map(|call| call["arguments"]["repo_path"].as_str().unwrap().to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(names, expected_names);
        assert_eq!(paths, expected_paths);
        assert!(
            calls
                .iter()
                .all(|call| call["arguments"].get("path").is_none())
        );
    });
}

#[test]
fn discovery_timeout_skips_indexing_in_every_index_mode_and_returns_promptly() {
    for index in [
        CodebaseMemoryIndex::Off,
        CodebaseMemoryIndex::Background,
        CodebaseMemoryIndex::Blocking,
    ] {
        let dir = fake_server_script();
        let workspace = tempfile::tempdir().expect("workspace");
        let log_path = workspace.path().join(format!("timeout-{index:?}.log"));
        let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
        let started = Instant::now();

        temper_agent_io::block_on(async move {
            let toolset = build_codebase_memory_toolset(
                Some(&config(
                    &dir,
                    CodebaseMemoryMode::Auto,
                    index,
                    "discovery-hang",
                    &log_path,
                    json!({}),
                )),
                "engineer",
                &context,
                workspace.path(),
            )
            .await
            .expect("auto mode retains read-only tools after unknown discovery");
            assert_eq!(toolset.status(), &CodebaseMemoryToolsetStatus::Started);
            let prompt = toolset.prompt_status().expect("prompt status");
            assert!(prompt.contains("discovery unavailable; indexing was not attempted"));
            assert!(prompt.contains("safe targeted project discovery was unavailable"));
            assert!(calls_named(&log_path, "index_repository").is_empty());

            let search = toolset
                .into_tools()
                .into_iter()
                .find(|tool| tool.name() == "codebase_memory_search_code")
                .expect("replacement read-only client is exposed");
            search
                .execute("search", json!({"query": "still available"}), None)
                .await
                .expect("replacement client remains usable");
        });
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "discovery timeout should remain bounded"
        );
    }
}

#[test]
fn malformed_or_unclassified_discovery_never_becomes_missing() {
    for server_mode in ["discovery-malformed", "discovery-error"] {
        let dir = fake_server_script();
        let workspace = tempfile::tempdir().expect("workspace");
        let log_path = workspace.path().join(format!("{server_mode}.log"));
        let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);

        temper_agent_io::block_on(async move {
            let toolset = build_codebase_memory_toolset(
                Some(&config(
                    &dir,
                    CodebaseMemoryMode::Auto,
                    CodebaseMemoryIndex::Blocking,
                    server_mode,
                    &log_path,
                    json!({}),
                )),
                "engineer",
                &context,
                workspace.path(),
            )
            .await
            .expect("auto mode reports unavailable discovery");
            assert!(
                toolset
                    .prompt_status()
                    .expect("prompt status")
                    .contains("discovery unavailable; indexing was not attempted")
            );
            assert!(calls_named(&log_path, "index_repository").is_empty());

            let required_error = match build_codebase_memory_toolset(
                Some(&config(
                    &dir,
                    CodebaseMemoryMode::Required,
                    CodebaseMemoryIndex::Blocking,
                    server_mode,
                    &log_path,
                    json!({}),
                )),
                "engineer",
                &context,
                workspace.path(),
            )
            .await
            {
                Ok(_) => panic!("required mode must reject unknown discovery"),
                Err(error) => error,
            };
            assert!(
                required_error
                    .to_string()
                    .contains("targeted codebase-memory discovery")
            );
            assert!(calls_named(&log_path, "index_repository").is_empty());
        });
    }
}

#[test]
fn incompatible_provider_versions_and_schemas_fail_safely_with_upgrade_guidance() {
    for server_mode in [
        "incompatible-name",
        "incompatible-version",
        "incompatible-capability",
        "incompatible-schema",
    ] {
        let dir = fake_server_script();
        let workspace = tempfile::tempdir().expect("workspace");
        let log_path = workspace.path().join(format!("{server_mode}.log"));
        let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);

        temper_agent_io::block_on(async move {
            let auto = build_codebase_memory_toolset(
                Some(&config(
                    &dir,
                    CodebaseMemoryMode::Auto,
                    CodebaseMemoryIndex::Blocking,
                    server_mode,
                    &log_path,
                    json!({}),
                )),
                "engineer",
                &context,
                workspace.path(),
            )
            .await
            .expect("auto mode disables an incompatible provider");
            assert!(matches!(
                auto.status(),
                CodebaseMemoryToolsetStatus::AutoUnavailable { reason }
                    if reason.contains("incompatible codebase-memory provider")
                        && reason.contains("upgrade `codebase-memory-mcp` to >= 0.9.0")
            ));
            assert!(auto.registered_tool_names().is_empty());

            let error = match build_codebase_memory_toolset(
                Some(&config(
                    &dir,
                    CodebaseMemoryMode::Required,
                    CodebaseMemoryIndex::Blocking,
                    server_mode,
                    &log_path,
                    json!({}),
                )),
                "engineer",
                &context,
                workspace.path(),
            )
            .await
            {
                Ok(_) => panic!("required mode must reject {server_mode}"),
                Err(error) => error,
            };
            assert!(
                error
                    .to_string()
                    .contains("upgrade `codebase-memory-mcp` to >= 0.9.0")
            );
            assert!(calls_named(&log_path, "index_repository").is_empty());
            assert!(calls_named(&log_path, "index_status").is_empty());
        });
    }
}

#[test]
fn blocking_index_timeout_is_mode_aware_and_does_not_damage_read_only_client() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("auto-index-timeout.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Auto,
                CodebaseMemoryIndex::Blocking,
                "index-hang",
                &log_path,
                json!({}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("auto indexing timeout keeps read-only provider available");
        assert!(
            toolset
                .prompt_status()
                .expect("prompt status")
                .contains("no path-keyed fallback was attempted")
        );
        let search = toolset
            .into_tools()
            .into_iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("search remains exposed");
        search
            .execute("search", json!({"query": "still works"}), None)
            .await
            .expect("index timeout was isolated from read-only client");
    });
}
