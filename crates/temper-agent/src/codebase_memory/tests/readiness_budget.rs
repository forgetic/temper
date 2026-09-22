use super::*;
use std::future::{Future, poll_fn};
use std::path::PathBuf;
use std::task::Poll;

struct ReleaseIndexOnDrop(PathBuf);

impl ReleaseIndexOnDrop {
    fn release(&self) {
        fs::write(&self.0, b"release").expect("release fixture indexing");
    }
}

impl Drop for ReleaseIndexOnDrop {
    fn drop(&mut self) {
        // Unblock the provider even when setup or a measured-call assertion panics.
        let _ = fs::write(&self.0, b"release");
    }
}

async fn wait_for_index_fixture(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "index fixture did not become ready"
        );
        temper_agent_io::sleep_for(Duration::from_millis(5)).await;
    }
}

#[test]
fn background_readiness_and_graph_execution_share_one_success_budget() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("background-budget-success.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
    let expected_project = super::super::scope::provider_key_for_repo(&context.repos[0]);
    let waiting_path = log_path.with_extension("index-waiting");
    let release = ReleaseIndexOnDrop(log_path.with_extension("index-release"));
    let mut fixture_config = config(
        &dir,
        CodebaseMemoryMode::Required,
        CodebaseMemoryIndex::Background,
        "background-budget-success",
        &log_path,
        json!({}),
    );
    let fixture_settings = fixture_config.codebase_memory.as_mut().unwrap();
    fixture_settings.startup_timeout_secs = 5;
    fixture_settings.index_timeout_secs = 15;

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset_with_timeout(
            Some(&fixture_config),
            "engineer",
            &context,
            workspace.path(),
            Duration::from_secs(5),
        )
        .await
        .expect("background indexing starts");
        let search = toolset
            .into_tools()
            .into_iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("search wrapper present");
        wait_for_index_fixture(&waiting_path).await;
        let mut execution = Box::pin(search.execute("search", json!({"query": "ready"}), None));
        poll_fn(|cx| {
            assert!(execution.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        // The call has entered readiness while the fixture still holds indexing.
        // Process startup happens before this measured wait, not in a timing race.
        temper_agent_io::sleep_for(Duration::from_millis(200)).await;
        release.release();
        let output = execution
            .await
            .expect("background readiness and graph call complete within one budget");
        assert!(!output.is_error);
        assert!(
            output.details.as_ref().unwrap()["timing"]["readiness_wait_ms"]
                .as_u64()
                .unwrap()
                >= 150
        );
        assert!(
            output.details.as_ref().unwrap()["timing"]["graph_execution_ms"]
                .as_u64()
                .unwrap()
                >= 25
        );

        let index_calls = calls_named(&log_path, "index_repository");
        let graph_calls = calls_named(&log_path, "search_code");
        assert_eq!(index_calls.len(), 1);
        assert_eq!(graph_calls.len(), 1);
        assert_eq!(graph_calls[0]["arguments"]["project"], expected_project);
        assert_ne!(index_calls[0]["pid"], graph_calls[0]["pid"]);
    });
}

#[test]
fn readiness_success_fixture_releases_indexing_when_assertions_panic() {
    let dir = tempfile::tempdir().expect("fixture directory");
    let path = dir.path().join("index-release");
    assert!(
        std::panic::catch_unwind(|| {
            let _release = ReleaseIndexOnDrop(path.clone());
            panic!("fixture assertion failed");
        })
        .is_err()
    );
    assert_eq!(fs::read(path).unwrap(), b"release");
}

#[test]
fn background_readiness_reduces_the_following_graph_rpc_budget() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("background-budget-timeout.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset_with_timeout(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Background,
                "background-budget-timeout",
                &log_path,
                json!({}),
            )),
            "engineer",
            &context,
            workspace.path(),
            Duration::from_millis(250),
        )
        .await
        .expect("background indexing starts");
        let search = toolset
            .into_tools()
            .into_iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("search wrapper present");
        let started = Instant::now();
        let output = search
            .execute("search", json!({"query": "bounded"}), None)
            .await
            .expect("deadline exhaustion is a typed tool output");
        assert!(output.is_error);
        assert!(
            matches!(
                output.details.as_ref().unwrap()[SAFE_TOOL_FAILURE_DETAIL_KEY]["category"].as_str(),
                Some("timeout" | "project_not_ready")
            ),
            "readiness budget exhaustion must remain a typed unavailable result"
        );
        assert!(
            started.elapsed() < Duration::from_millis(400),
            "readiness and RPC must not each receive the full timeout"
        );
        let timing = &output.details.as_ref().unwrap()["timing"];
        assert!(timing["duration_ms"].as_u64().unwrap() < 400);
    });
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
