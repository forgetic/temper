use super::*;
use std::path::Path;

#[test]
fn discovery_remains_admitted_until_fresh_serving_contract_is_validated() {
    let fixture = handoff_server();
    let workspace = tempfile::tempdir().unwrap();
    let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
    let log = workspace.path().join("handoff.log");
    let checked_log = log.clone();
    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(
                &fixture,
                CodebaseMemoryMode::Required,
                CodebaseMemoryIndex::Off,
                "handoff",
                &log,
                json!({}),
            )),
            "engineer",
            &context,
            workspace.path(),
        )
        .await
        .expect("healthy handoff");
        let admissions = calls_named(&log, "session.list_tools");
        assert_eq!(admissions.len(), 2);
        assert_ne!(admissions[0]["pid"], admissions[1]["pid"]);
        assert_eq!(admissions[1]["arguments"]["previous_alive"], true);
        let discovery = admissions[0]["pid"].as_u64().unwrap();
        wait_for_absence(&[discovery]);
        drop(toolset);
        let serving = admissions[1]["pid"].as_u64().unwrap();
        wait_for_absence(&[serving]);
    });
    assert_eq!(calls_named(&checked_log, "index_repository").len(), 0);
}

#[test]
fn serving_partial_failures_clean_both_frontends_in_auto_and_required_modes() {
    for mode in [CodebaseMemoryMode::Auto, CodebaseMemoryMode::Required] {
        for failure in [
            "serving-connect-exit",
            "serving-list-timeout",
            "serving-contract-invalid",
        ] {
            let fixture = handoff_server();
            let workspace = tempfile::tempdir().unwrap();
            let context = workspace_context(workspace.path(), &[("acme", "demo", "demo")]);
            let log = workspace.path().join("handoff.log");
            temper_agent_io::block_on(async move {
                let started = Instant::now();
                let result = build_codebase_memory_toolset(
                    Some(&config(
                        &fixture,
                        mode,
                        CodebaseMemoryIndex::Off,
                        failure,
                        &log,
                        json!({}),
                    )),
                    "engineer",
                    &context,
                    workspace.path(),
                )
                .await;
                assert_eq!(result.is_ok(), mode == CodebaseMemoryMode::Auto);
                drop(result);
                let pids = calls_named(&log, "session.initialize")
                    .iter()
                    .map(|entry| entry["pid"].as_u64().unwrap())
                    .collect::<Vec<_>>();
                assert_eq!(pids.len(), 2);
                wait_for_absence(&pids);
                assert!(started.elapsed() < Duration::from_secs(4));
            });
        }
    }
}

fn handoff_server() -> tempfile::TempDir {
    let fixture = fake_server_script();
    let path = fixture.path().join("fake_codebase_memory_mcp.py");
    let source = fs::read_to_string(&path).unwrap();
    let source = source.replace("    if method == \"initialize\":\n", r#"    if method == "initialize":
        log_tool("session.initialize", {})
        with open(log_path, encoding="utf-8") as handle:
            sessions = [json.loads(line)["pid"] for line in handle if json.loads(line)["name"] == "session.initialize"]
        if len(sessions) == 2 and mode == "serving-connect-exit":
            sys.exit(1)
"#);
    let source = source.replace(
        "    elif method == \"tools/list\":\n",
        r#"    elif method == "tools/list":
        if len(sessions) == 2:
            time.sleep(0.1)
        previous_alive = len(sessions) == 1 or os.path.exists(f"/proc/{sessions[0]}")
        log_tool("session.list_tools", {"previous_alive": previous_alive})
        if len(sessions) == 2 and mode == "serving-list-timeout":
            time.sleep(60)
        if len(sessions) == 2 and mode == "serving-contract-invalid":
            TOOLS = []
"#,
    );
    assert!(source.contains("previous_alive"));
    fs::write(path, source).unwrap();
    fixture
}

fn wait_for_absence(pids: &[u64]) {
    let started = Instant::now();
    while pids
        .iter()
        .any(|pid| Path::new(&format!("/proc/{pid}")).exists())
    {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "frontend cleanup remained pending"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
