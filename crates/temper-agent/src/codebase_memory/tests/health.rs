use super::test_support::*;
use super::*;
use serde_json::json;
use std::time::{Duration, Instant};
use temper_protocol_agent::{CodebaseMemoryIndex, CodebaseMemoryMode};
use tongs::tools::ToolOutput;

fn category(output: &ToolOutput) -> Option<&str> {
    output
        .details
        .as_ref()?
        .get(SAFE_TOOL_FAILURE_DETAIL_KEY)?
        .get("category")?
        .as_str()
}

#[test]
fn systemic_failure_opens_every_wrapper_and_a_new_toolset_resets_health() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("systemic-circuit.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
    let tool_config = config(
        &dir,
        CodebaseMemoryMode::Required,
        CodebaseMemoryIndex::Off,
        "graph-errors",
        &log_path,
        json!({}),
    );

    temper_agent_io::block_on(async move {
        let first = build_codebase_memory_toolset(
            Some(&tool_config),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("first toolset starts");
        let first_tools = first.into_tools();
        let search = first_tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("search wrapper");
        let architecture = first_tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_get_architecture")
            .expect("architecture wrapper");
        let list = first_tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_list_projects")
            .expect("list wrapper");

        let failed = search
            .execute("systemic", json!({"query": "systemic"}), None)
            .await
            .expect("provider failure is typed");
        assert_eq!(category(&failed), Some("provider_protocol"));
        let rendered = format!(
            "{} {}",
            output_text(&failed),
            serde_json::to_string(&failed.details).unwrap()
        );
        assert!(rendered.contains("conventional discovery"));
        assert!(rendered.contains("do not retry codebase-memory immediately"));
        assert!(!rendered.contains("SECRET"));
        assert!(!rendered.contains("unusable provider state"));

        for (call_id, tool) in [("architecture", architecture), ("list", list)] {
            let output = tool
                .execute(call_id, json!({}), None)
                .await
                .expect("open circuit is a typed result");
            assert_eq!(category(&output), Some("circuit_open"));
            assert!(output_text(&output).contains("disabled for this run"));
            assert_eq!(
                output.details.as_ref().unwrap()["circuit"]["scope"],
                "codebase_memory_toolset_run"
            );
            assert_eq!(
                output.details.as_ref().unwrap()["circuit"]["opened_by"],
                "provider_protocol"
            );
        }
        let circuit = ToolFailureDiagnostic::codebase_memory(ToolFailureCategory::CircuitOpen);
        assert!(!circuit.retryable);
        assert!(circuit.fallback_to_conventional_discovery);
        assert!(!circuit.message.contains("SECRET"));
        assert_eq!(calls_named(&log_path, "search_code").len(), 1);
        assert!(calls_named(&log_path, "get_architecture").is_empty());
        drop(first_tools);

        let second = build_codebase_memory_toolset(
            Some(&tool_config),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("second run gets fresh health state");
        let second_tools = second.into_tools();
        let search = second_tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("second search wrapper");
        let output = search
            .execute("healthy-new-run", json!({"query": "healthy"}), None)
            .await
            .expect("new run reaches MCP");
        assert!(!output.is_error);
        assert_eq!(calls_named(&log_path, "search_code").len(), 2);
    });
}

#[test]
fn parallel_wrappers_recheck_health_before_reaching_the_serialized_client() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("parallel-circuit.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Off,
                "graph-systemic",
                &log_path,
                json!({}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("toolset starts");
        let tools = toolset.into_tools();
        let search = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("search wrapper");
        let architecture = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_get_architecture")
            .expect("architecture wrapper");

        let (first, second) = futures::join!(
            search.execute("parallel-search", json!({"query": "needle"}), None),
            architecture.execute("parallel-architecture", json!({}), None),
        );
        let first = first.expect("first result");
        let second = second.expect("second result");
        let mut categories = [category(&first), category(&second)];
        categories.sort_unstable();
        assert_eq!(
            categories,
            [Some("circuit_open"), Some("provider_protocol")]
        );
        assert_eq!(
            calls_named(&log_path, "search_code").len()
                + calls_named(&log_path, "get_architecture").len(),
            1,
            "only the first parallel wrapper may enter MCP"
        );
    });
}

#[test]
fn query_local_failures_and_empty_results_keep_the_circuit_healthy() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("local-errors.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Off,
                "graph-errors",
                &log_path,
                json!({}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("toolset starts");
        let tools = toolset.into_tools();
        let search = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("search wrapper");

        let invalid = search
            .execute("invalid", json!({"query": "invalid"}), None)
            .await
            .expect("provider validation failure is typed");
        assert_eq!(category(&invalid), Some("invalid_model_input"));
        assert!(!output_text(&invalid).contains("SECRET"));

        let oversized = search
            .execute(
                "oversized",
                json!({"query": "x".repeat(MAX_MCP_RECORD_BYTES)}),
                None,
            )
            .await
            .expect("oversized input is rejected before MCP");
        assert_eq!(category(&oversized), Some("invalid_model_input"));

        let empty = search
            .execute("empty", json!({"query": "empty"}), None)
            .await
            .expect("ordinary empty result succeeds");
        assert!(!empty.is_error);
        assert!(output_text(&empty).is_empty());

        let healthy = search
            .execute("healthy", json!({"query": "healthy"}), None)
            .await
            .expect("healthy client remains callable");
        assert!(!healthy.is_error);
        assert_eq!(calls_named(&log_path, "search_code").len(), 3);
    });
}

#[test]
fn timeout_opens_the_circuit_without_a_second_request_or_live_process() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("timeout-circuit.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset_with_timeout(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Off,
                "graph-timeout",
                &log_path,
                json!({}),
            )),
            "engineer",
            &context,
            workspace.path(),
            Duration::from_millis(200),
        )
        .await
        .expect("toolset starts");
        let tools = toolset.into_tools();
        let search = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("search wrapper");
        let architecture = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_get_architecture")
            .expect("architecture wrapper");

        let started = Instant::now();
        let timeout = search
            .execute("timeout", json!({"query": "hang"}), None)
            .await
            .expect("timeout is typed");
        assert_eq!(category(&timeout), Some("timeout"));
        assert!(started.elapsed() < Duration::from_secs(1));
        let first = ToolFailureDiagnostic::codebase_memory(ToolFailureCategory::Timeout);
        assert!(first.retryable);

        let immediate = Instant::now();
        let open = architecture
            .execute("after-timeout", json!({}), None)
            .await
            .expect("later wrapper returns immediately");
        assert_eq!(category(&open), Some("circuit_open"));
        assert!(immediate.elapsed() < Duration::from_millis(100));
        let calls = calls_named(&log_path, "search_code");
        assert_eq!(calls.len(), 1);
        assert!(calls_named(&log_path, "get_architecture").is_empty());
        wait_for_process_exit(calls[0]["pid"].as_u64().unwrap() as u32);
    });
}

#[test]
fn failed_background_index_opens_before_any_graph_request() {
    let dir = fake_server_script();
    let workspace = tempfile::tempdir().expect("workspace");
    let log_path = workspace.path().join("index-failure-circuit.log");
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);

    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &dir,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Background,
                "index-error",
                &log_path,
                json!({}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("background index starts");
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
            .execute("index-failed", json!({"query": "needle"}), None)
            .await
            .expect("index failure is typed");
        assert_eq!(category(&failed), Some("index_failure"));
        let open = architecture
            .execute("after-index-failure", json!({}), None)
            .await
            .expect("shared circuit opens");
        assert_eq!(category(&open), Some("circuit_open"));
        assert!(calls_named(&log_path, "search_code").is_empty());
        assert!(calls_named(&log_path, "get_architecture").is_empty());
    });
}
