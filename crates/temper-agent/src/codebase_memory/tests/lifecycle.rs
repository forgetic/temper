use super::*;
use std::time::{Duration, Instant};
use temper_protocol_agent::WorkspaceContext;

fn provider_key(context: &WorkspaceContext, index: usize) -> String {
    super::super::scope::provider_key_for_repo(&context.repos[index])
}

#[test]
fn hundreds_of_historical_projects_do_not_force_global_inventory_or_grow_the_cache() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("scale.log");
    let context = workspace_context(workspace.path(), &[("acme", "app", "app")]);
    let stable_key = provider_key(&context, 0);
    let mut projects = (0..500)
        .map(|index| {
            json!({
                "project": format!("historical-{index:04}"),
                "repo_path": format!("/retired/engineer/{index:04}/temper"),
                "status": "fresh",
                "updated_at_unix_secs": index + 1,
                "estimated_bytes": 1024,
            })
        })
        .collect::<Vec<_>>();
    projects.push(json!({
        "project": stable_key,
        "repo_path": workspace.path().join("prior-checkout"),
        "status": "fresh",
        "updated_at_unix_secs": 999,
        "estimated_bytes": 4096,
    }));
    let seed = json!({"cache_instance_id": "scale-cache", "projects": projects});
    let run_config = config_with_args(
        &dir,
        CodebaseMemoryMode::Required,
        CodebaseMemoryIndex::Blocking,
        "stateful",
        &log_path,
        seed,
        &["--delay-list-ms", "5000", "--evidence-limit", "8"],
    );
    let cwd = workspace.path().to_path_buf();
    let started = Instant::now();
    temper_agent_io::block_on(async move {
        build_codebase_memory_toolset(Some(&run_config), "engineer", &context, &cwd)
            .await
            .expect("targeted lookup remains independent of slow global inventory");
    });
    let snapshot = provider_snapshot(&dir);
    assert_eq!(snapshot["projects"].as_object().unwrap().len(), 501);
    assert_eq!(snapshot["counters"]["index_status"], 1);
    assert!(snapshot["counters"].get("list_projects").is_none());
    assert!(snapshot["counters"].get("index_repository").is_none());
    assert!(snapshot["evidence"].as_array().unwrap().len() <= 8);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "targeted startup must not pay five-second inventory latency"
    );
}

#[test]
fn persistent_state_reuses_one_project_across_roots_and_separates_repositories() {
    let dir = fake_server_script();
    let first = tempfile::tempdir().expect("first workspace");
    let second = tempfile::tempdir().expect("second workspace");
    let log_path = first.path().join("persistent.log");
    let first_context = workspace_context(first.path(), &[("acme", "app", "app")]);
    let second_context = workspace_context(
        second.path(),
        &[("acme", "app", "other-app-root"), ("acme", "lib", "lib")],
    );
    let app_key = provider_key(&first_context, 0);
    let lib_key = provider_key(&second_context, 1);
    assert_eq!(app_key, provider_key(&second_context, 0));
    assert_ne!(app_key, lib_key);

    let first_config = config(
        &dir,
        CodebaseMemoryMode::Required,
        CodebaseMemoryIndex::Blocking,
        "stateful",
        &log_path,
        json!({}),
    );
    let first_cwd = first.path().to_path_buf();
    let first_run_context = first_context.clone();
    temper_agent_io::block_on(async move {
        build_codebase_memory_toolset(
            Some(&first_config),
            "engineer",
            &first_run_context,
            &first_cwd,
        )
        .await
        .expect("first checkout creates its stable project");
    });
    let first_snapshot = provider_snapshot(&dir);
    assert_eq!(first_snapshot["projects"].as_object().unwrap().len(), 1);
    assert_eq!(first_snapshot["counters"]["index_repository"], 1);

    let second_config = config(
        &dir,
        CodebaseMemoryMode::Required,
        CodebaseMemoryIndex::Blocking,
        "stateful",
        &log_path,
        json!({}),
    );
    let second_cwd = second.path().to_path_buf();
    temper_agent_io::block_on(async move {
        build_codebase_memory_toolset(
            Some(&second_config),
            "engineer",
            &second_context,
            &second_cwd,
        )
        .await
        .expect("new checkout reuses app and creates only distinct lib");
    });

    let snapshot = provider_snapshot(&dir);
    let records = snapshot["projects"].as_object().unwrap();
    assert_eq!(records.len(), 2);
    assert!(records.contains_key(&app_key));
    assert!(records.contains_key(&lib_key));
    assert_eq!(snapshot["counters"]["index_repository"], 2);
    assert_eq!(snapshot["counters"]["project_creations"], 2);
    assert_eq!(calls_named(&log_path, "index_status").len(), 3);
    assert!(calls_named(&log_path, "list_projects").is_empty());
    assert_eq!(
        records[&app_key]["repo_path"],
        first
            .path()
            .join("app")
            .canonicalize()
            .unwrap()
            .display()
            .to_string(),
        "reused identity must not be rewritten merely because the checkout root changed"
    );
}

#[test]
fn targeted_timeout_preserves_persistent_state_and_does_not_amplify_the_next_session() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("persistent-timeout.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
    let stable_key = provider_key(&context, 0);
    let seed = json!({"projects": [{
        "project": stable_key,
        "repo_path": workspace.path().join("old-root"),
        "status": "fresh",
        "updated_at_unix_secs": 1,
    }]});

    let timeout_config = config_with_args(
        &dir,
        CodebaseMemoryMode::Auto,
        CodebaseMemoryIndex::Blocking,
        "stateful",
        &log_path,
        seed,
        &["--delay-status-ms", "1500"],
    );
    let timeout_cwd = workspace.path().to_path_buf();
    let timeout_context = context.clone();
    let started = Instant::now();
    temper_agent_io::block_on(async move {
        build_codebase_memory_toolset(
            Some(&timeout_config),
            "engineer",
            &timeout_context,
            &timeout_cwd,
        )
        .await
        .expect("auto mode returns after its targeted discovery deadline");
    });
    assert!(started.elapsed() < Duration::from_secs(3));
    let after_timeout = provider_snapshot(&dir);
    assert_eq!(after_timeout["projects"].as_object().unwrap().len(), 1);
    assert!(after_timeout["counters"].get("index_repository").is_none());
    assert!(after_timeout["counters"].get("delete_project").is_none());

    let recovered_config = config(
        &dir,
        CodebaseMemoryMode::Required,
        CodebaseMemoryIndex::Blocking,
        "stateful",
        &log_path,
        json!({}),
    );
    let recovered_cwd = workspace.path().to_path_buf();
    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&recovered_config),
            "engineer",
            &context,
            &recovered_cwd,
        )
        .await
        .expect("next session promptly reuses unchanged persistent state");
        assert!(toolset.prompt_status().unwrap().contains("fresh/non-stale"));
    });
    let recovered = provider_snapshot(&dir);
    assert_eq!(recovered["projects"].as_object().unwrap().len(), 1);
    assert_eq!(recovered["counters"]["index_status"], 2);
    assert!(recovered["counters"].get("index_repository").is_none());
    assert!(recovered["counters"].get("delete_project").is_none());
}

#[test]
fn concurrent_fake_provider_processes_stably_upsert_one_identity() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
    let stable_key = provider_key(&context, 0);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));

    std::thread::scope(|scope| {
        for index in 0..4 {
            let barrier = std::sync::Arc::clone(&barrier);
            let script = script_path(&dir);
            let state = provider_state_path(&dir);
            let log = workspace.path().join("concurrent.log");
            let project = stable_key.clone();
            let root = workspace.path().join(format!("checkout-{index}"));
            scope.spawn(move || {
                use std::io::Write as _;
                barrier.wait();
                let mut child = std::process::Command::new("python3")
                    .args([
                        "-u",
                        &script.display().to_string(),
                        "--state",
                        &state.display().to_string(),
                        "--log",
                        &log.display().to_string(),
                        "--mode",
                        "stateful",
                    ])
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .expect("spawn concurrent fake provider");
                let request = json!({
                    "jsonrpc": "2.0",
                    "id": index + 1,
                    "method": "tools/call",
                    "params": {
                        "name": "index_repository",
                        "arguments": {"name": project, "repo_path": root},
                    },
                });
                writeln!(child.stdin.take().unwrap(), "{request}").unwrap();
                assert!(child.wait().unwrap().success());
            });
        }
    });

    let snapshot = provider_snapshot(&dir);
    assert_eq!(snapshot["projects"].as_object().unwrap().len(), 1);
    assert!(snapshot["projects"].get(&stable_key).is_some());
    assert_eq!(snapshot["counters"]["index_repository"], 4);
    assert_eq!(snapshot["counters"]["project_creations"], 1);
    assert_eq!(snapshot["counters"]["upsert_writes"], 4);
}
