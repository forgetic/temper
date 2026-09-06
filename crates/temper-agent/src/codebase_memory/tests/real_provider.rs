//! Manual release smoke: uses the installed binary and an isolated provider runtime.

use super::*;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

const PROVIDER_VERSION: &str = "0.10.8";
const TIMEOUT: Duration = Duration::from_secs(60);

struct ProviderRuntime {
    directory: tempfile::TempDir,
    args: Vec<String>,
}

impl ProviderRuntime {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("temper-cbm-")
            .tempdir_in("/tmp")
            .expect("private provider directory");
        let mut args = Vec::new();
        for (variable, child) in [("CBM_RUNTIME_DIR", "runtime"), ("CBM_CACHE_DIR", "cache")] {
            let path = directory.path().join(child);
            fs::create_dir(&path).expect("provider directory");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .expect("owner-only provider directory");
            args.push(format!("{variable}={}", path.display()));
        }
        args.push("codebase-memory-mcp".to_string());
        Self { directory, args }
    }

    fn mcp_config(&self) -> StdioMcpServerConfig {
        StdioMcpServerConfig::new("env", self.args.clone())
            .with_startup_timeout(Duration::from_secs(30))
            .with_call_timeout(TIMEOUT)
    }

    fn agent_config(&self) -> AgentToolConfig {
        AgentToolConfig {
            codebase_memory: Some(CodebaseMemoryToolConfig {
                mode: CodebaseMemoryMode::Required,
                command: "env".to_string(),
                args: self.args.clone(),
                roles: vec!["engineer".to_string()],
                index: CodebaseMemoryIndex::Blocking,
                startup_timeout_secs: 30,
                index_timeout_secs: TIMEOUT.as_secs(),
                retention: Default::default(),
            }),
        }
    }
}

impl Drop for ProviderRuntime {
    fn drop(&mut self) {
        // Both overrides remain in force even during panic cleanup: never stop
        // the account's production daemon or remove its cache.
        let _ = Command::new("env")
            .args(&self.args)
            .args(["daemon", "stop"])
            .output();
    }
}

#[test]
#[ignore = "requires installed codebase-memory-mcp 0.10.8; see docs/how-to/upgrade-codebase-memory.md"]
fn installed_provider_release_supports_temper_graph_tools() {
    let runtime = ProviderRuntime::new();
    let context = workspace_context(runtime.directory.path(), &[("acme", "demo", "demo")]);
    let repo = runtime.directory.path().join("demo");
    fs::write(
        repo.join("example.py"),
        "def increment(value):\n    return value + 1\n\ndef run():\n    return increment(41)\n",
    )
    .expect("fixture source");
    fs::write(repo.join(".cbmignore"), "excluded.py\n").expect("controlled exclusion");
    fs::write(repo.join("excluded.py"), "def excluded():\n    return 7\n")
        .expect("excluded source");
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(&repo)
            .status()
            .unwrap()
            .success()
    );

    temper_agent_io::block_on(async move {
        let client = StdioMcpClient::connect(runtime.mcp_config())
            .await
            .expect("provider starts");
        let metadata = client.server_metadata().expect("provider metadata");
        assert_eq!(metadata.version.as_deref(), Some(PROVIDER_VERSION));
        let advertised = client
            .list_tools(TIMEOUT)
            .await
            .expect("provider tool schemas");
        validate_provider_contract(&client, &advertised).expect("supported provider contract");
        drop(client);

        let config = runtime.agent_config();
        let toolset = build_codebase_memory_toolset(
            Some(&config),
            "engineer",
            &context,
            runtime.directory.path(),
        )
        .await
        .expect("real provider indexes and confirms the active checkout");
        assert!(
            toolset
                .prompt_status()
                .unwrap()
                .contains("stable current-checkout rebind completed")
        );
        let tools = toolset.into_tools();
        verify_graph_tools(&tools).await;
        let first = call(
            &tools,
            "check_index_coverage",
            json!({"paths":["example.py","excluded.py"],"scopes":["."],"scope_limit":1}),
        )
        .await;
        let report: Value = serde_json::from_str(&first).expect("coverage report");
        assert_eq!(report["status"], "flagged", "{first}");
        assert_eq!(
            report["provider"]["paths"][0]["freshness"],
            "metadata_match"
        );
        assert_ne!(
            report["provider"]["paths"][1]["status"],
            "no_recorded_issue"
        );
        let last=call(&tools,"check_index_coverage",json!({"paths":["example.py","excluded.py"],"scopes":["."],"scope_limit":1,"scope_offset":report["next_scope_offset"],"generation":report["generation"]})).await;
        let last: Value = serde_json::from_str(&last).unwrap();
        assert_eq!(last["status"], "flagged");
        assert_eq!(last["pagination_complete"], true, "{last}");
        assert_eq!(last["scope_pages"].as_array().unwrap().len(), 2);
        fs::write(
            repo.join("example.py"),
            "def increment(value):\n    return value + 2\n",
        )
        .expect("controlled source change");
        let changed = call(
            &tools,
            "check_index_coverage",
            json!({"paths":["example.py"],"generation":report["generation"]}),
        )
        .await;
        let changed: Value = serde_json::from_str(&changed).unwrap();
        assert_eq!(changed["status"], "stale", "{changed}");
        println!(
            "installed_provider_0_10_8 coverage_paths_scopes_exclusion_changed_file=passed generation={} pagination_complete={}",
            report["generation"], report["pagination_complete"]
        );
    });
}

async fn verify_graph_tools(tools: &[Box<dyn Tool>]) {
    let graph = call(
        tools,
        "search_graph",
        json!({"name_pattern": "^increment$"}),
    )
    .await;
    // The model-visible JSON is followed by Temper's decision guidance.
    let graph = serde_json::Deserializer::from_str(&graph)
        .into_iter::<Value>()
        .next()
        .expect("graph result")
        .expect("graph JSON");
    assert_eq!(graph["results"][0]["name"], "increment");
    let symbol = graph["results"][0]["qualified_name"]
        .as_str()
        .expect("indexed symbol");
    let snippet = call(tools, "get_code_snippet", json!({"qualified_name": symbol})).await;
    assert!(snippet.contains("return value + 1"), "{snippet}");
    let trace = call(
        tools,
        "trace_path",
        json!({"function_name": symbol, "direction": "inbound"}),
    )
    .await;
    assert!(trace.contains("run"), "{trace}");
    let search = call(tools, "search_code", json!({"pattern": "increment"})).await;
    assert!(search.contains("example.py"), "{search}");
}

async fn call(tools: &[Box<dyn Tool>], name: &str, args: Value) -> String {
    let name = format!("codebase_memory_{name}");
    let tool = tools
        .iter()
        .find(|tool| tool.name() == name)
        .expect("registered tool");
    let output = tool
        .execute("release-smoke", args, None)
        .await
        .expect("tool execution");
    assert!(!output.is_error, "{}: {}", name, output_text(&output));
    if name == "codebase_memory_search_graph" || name == "codebase_memory_get_code_snippet" {
        assert!(
            output
                .details
                .as_ref()
                .and_then(|details| details
                    .get(temper_agent_core::SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY))
                .is_some(),
            "{name} must retain typed source-selection evidence"
        );
    }
    output_text(&output)
}
