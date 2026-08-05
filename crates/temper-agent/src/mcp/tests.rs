use super::*;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio as StdStdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static PROCESS_TEST_LOCK: Mutex<()> = Mutex::new(());

fn fake_server_script() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
            dir.path().join("fake_mcp.py"),
            r#"
import json
import os
import subprocess
import sys
import time

mode = sys.argv[1] if len(sys.argv) > 1 else "normal"
if mode == "hang":
    if len(sys.argv) > 2:
        with open(sys.argv[2], "w") as pid_file:
            pid_file.write(str(os.getpid()))
    time.sleep(60)
    sys.exit(0)

TOOLS = [
    {"name": "search_code", "description": "Search code", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}},
    {"name": "get_architecture", "description": "Architecture", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "delete_project", "description": "Danger", "inputSchema": {"type": "object", "properties": {}}},
]

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    if mode == "oversized_record":
        sys.stdout.write("x" * (1024 * 1024 + 1) + "\n")
        sys.stdout.flush()
        time.sleep(60)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"protocolVersion": "2024-11-05", "serverInfo": {"name": "fake", "version": "1"}, "capabilities": {"tools": {}}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"tools": TOOLS}})
    elif method == "tools/call":
        params = request.get("params", {})
        name = params.get("name")
        args = params.get("arguments") or {}
        if mode == "hang_call":
            time.sleep(60)
        if mode == "grandchild":
            grandchild = subprocess.Popen(["sleep", "60"], start_new_session=True)
            with open(sys.argv[2], "w") as pid_file:
                pid_file.write(str(grandchild.pid))
            time.sleep(60)
        if mode == "server_exit":
            grandchild = subprocess.Popen(["sleep", "60"], start_new_session=True)
            with open(sys.argv[2], "w") as pid_file:
                pid_file.write(str(grandchild.pid))
            sys.exit(23)
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"content": [{"type": "text", "text": f"called {name} with {json.dumps(args, sort_keys=True)}"}], "isError": False}})
    else:
        send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown method"}})
"#,
        )
        .expect("write fake server");
    dir
}

fn fake_command_with_extra(
    dir: &tempfile::TempDir,
    mode: Option<&str>,
    extra: Option<String>,
) -> StdioMcpServerConfig {
    let mut args = vec!["-u".to_string(), script_path(dir).display().to_string()];
    if let Some(mode) = mode {
        args.push(mode.to_string());
    }
    if let Some(extra) = extra {
        args.push(extra);
    }
    StdioMcpServerConfig::new("python3", args)
        .with_startup_timeout(Duration::from_secs(1))
        .with_call_timeout(Duration::from_secs(2))
}

fn fake_command(dir: &tempfile::TempDir, mode: Option<&str>) -> StdioMcpServerConfig {
    fake_command_with_extra(dir, mode, None)
}

async fn connect(config: StdioMcpServerConfig) -> Result<StdioMcpClient, McpError> {
    StdioMcpClient::connect_with_containment(
        config,
        crate::containment_tests::containment_context(),
    )
    .await
}

fn script_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("fake_mcp.py")
}

fn process_exists(pid: u32) -> bool {
    StdCommand::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(StdStdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn process_alive(pid: u32) -> bool {
    if !process_exists(pid) {
        return false;
    }
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| stat.rsplit_once(") ").map(|(_, rest)| rest.to_string()))
        .and_then(|rest| rest.chars().next())
        .is_some_and(|state| state != 'Z')
}

fn assert_reader_joined(config: &StdioMcpServerConfig) {
    assert!(
        super::connection::output_reader_joined(config),
        "terminal MCP result preceded output-reader join"
    );
}

fn wait_for_cleanup(config: &StdioMcpServerConfig, pids: &[u32]) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while pids.iter().any(|pid| process_alive(*pid))
        || !super::connection::output_reader_joined(config)
    {
        assert!(Instant::now() < deadline, "MCP cleanup owner did not join");
        std::thread::yield_now();
    }
}

async fn wait_for_pid(path: &Path, description: &str) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(value) = fs::read_to_string(path) {
            if let Ok(pid) = value.trim().parse() {
                return pid;
            }
        }
        assert!(Instant::now() < deadline, "{description} was not published");
        temper_agent_io::sleep_for(Duration::from_millis(5)).await;
    }
}

#[test]
fn containment_identity_never_uses_mcp_command_or_arguments() {
    let config = StdioMcpServerConfig::new(
        "credential=secret-token-sentinel",
        vec!["--token=secret-token-sentinel".to_string()],
    );
    assert_eq!(config.containment_identity(), "mcp-server");
    assert_eq!(
        config
            .with_containment_identity("codebase-memory")
            .containment_identity(),
        "codebase-memory"
    );
}

#[test]
fn emergency_hard_kill_bypasses_a_blocked_mcp_request_mutex() {
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = fake_server_script();
    let config = fake_command(&dir, Some("hang_call"));
    temper_agent_io::block_on(async move {
        let containment = crate::containment_tests::containment_context();
        let emergency = containment.emergency_termination_registry();
        let client = StdioMcpClient::connect_with_containment(config.clone(), containment)
            .await
            .expect("connect fake MCP server");
        let pid = client.child_id();
        let request = client.call_tool("blocked", json!({}), Duration::from_secs(30));
        let kill = async move {
            temper_agent_io::sleep_for(Duration::from_millis(25)).await;
            let receipt = emergency.request_hard_kill();
            assert_eq!(receipt.requested_count(), 1);
        };
        let completed =
            temper_agent_io::timeout(Duration::from_secs(2), futures::future::join(request, kill))
                .await
                .expect("emergency KILL must settle blocked MCP request");
        assert!(completed.0.is_err());
        wait_for_cleanup(&config, &[pid]);
    });
}

#[test]
fn cancellation_wakes_a_request_mutex_waiter_and_joins_both_operations() {
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = fake_server_script();
    let config = fake_command(&dir, Some("hang_call"));
    temper_agent_io::block_on(async move {
        let client = connect(config.clone())
            .await
            .expect("connect fake MCP server");
        let pid = client.child_id();
        let first = client.call_tool("first", json!({}), Duration::from_secs(30));
        let second = client.call_tool("second", json!({}), Duration::from_secs(30));
        let outcome = temper_agent_io::timeout(
            Duration::from_millis(100),
            futures::future::join(first, second),
        )
        .await;
        assert!(outcome.is_err(), "generic cancellation must win");
        wait_for_cleanup(&config, &[pid]);
    });
}

#[test]
fn cancellation_reaps_the_mcp_server_grandchild_group() {
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = fake_server_script();
    let pid_path = dir.path().join("grandchild.pid");
    let config = fake_command_with_extra(
        &dir,
        Some("grandchild"),
        Some(pid_path.display().to_string()),
    );
    temper_agent_io::block_on(async move {
        let client = connect(config.clone())
            .await
            .expect("connect fake MCP server");
        let server_pid = client.child_id();
        let call = Box::pin(client.call_tool("hang", json!({}), Duration::from_secs(30)));
        let published_pid = Box::pin(wait_for_pid(&pid_path, "grandchild pid"));
        let (grandchild_pid, call) = match futures::future::select(call, published_pid).await {
            futures::future::Either::Left((result, _)) => {
                panic!("MCP call completed before grandchild started: {result:?}")
            }
            futures::future::Either::Right((pid, call)) => (pid, call),
        };
        let outcome = temper_agent_io::timeout(Duration::from_millis(100), call).await;
        assert!(outcome.is_err());
        wait_for_cleanup(&config, &[server_pid, grandchild_pid]);
    });
}

#[test]
fn request_timeout_waits_for_recursive_cleanup_and_reader_join() {
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = fake_server_script();
    let config = fake_command(&dir, Some("hang_call"));
    temper_agent_io::block_on(async move {
        let client = connect(config.clone())
            .await
            .expect("connect fake MCP server");
        let pid = client.child_id();
        let error = client
            .call_tool("timeout", json!({}), Duration::from_millis(75))
            .await
            .expect_err("request must time out");
        assert!(matches!(error, McpError::Timeout { .. }));
        assert!(!process_alive(pid), "timeout result preceded cleanup proof");
        assert_reader_joined(&config);
    });
}

#[test]
fn server_exit_with_new_session_waits_for_recursive_cleanup_and_reader_join() {
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = fake_server_script();
    let pid_path = dir.path().join("server-exit-descendant.pid");
    let config = fake_command_with_extra(
        &dir,
        Some("server_exit"),
        Some(pid_path.display().to_string()),
    );
    temper_agent_io::block_on(async move {
        let client = connect(config.clone())
            .await
            .expect("connect fake MCP server");
        let server_pid = client.child_id();
        let error = client
            .call_tool("exit", json!({}), Duration::from_secs(2))
            .await
            .expect_err("server exits without a response");
        assert!(matches!(error, McpError::ProcessExited { .. }));
        let descendant = fs::read_to_string(&pid_path)
            .expect("new-session descendant pid")
            .parse()
            .expect("numeric pid");
        assert!(
            !process_alive(server_pid),
            "server result preceded cleanup proof"
        );
        assert!(
            !process_alive(descendant),
            "new-session descendant survived server failure"
        );
        assert_reader_joined(&config);
    });
}

#[test]
fn oversized_inbound_record_is_typed_and_joins_server() {
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = fake_server_script();
    let config = fake_command(&dir, Some("oversized_record"));
    let connection_config = config.clone();
    let error = temper_agent_io::block_on(async move {
        match connect(connection_config).await {
            Ok(_) => panic!("oversized MCP record must fail"),
            Err(error) => error,
        }
    });
    assert!(matches!(
        error,
        McpError::ProtocolOverflow {
            direction: "inbound",
            resource: "record bytes",
            limit: super::connection::MAX_MCP_RECORD_BYTES,
            ..
        }
    ));
    assert_reader_joined(&config);
}

#[test]
fn oversized_outbound_record_is_typed_and_contains_server() {
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = fake_server_script();
    let config = fake_command(&dir, None);
    temper_agent_io::block_on(async move {
        let client = connect(config.clone())
            .await
            .expect("connect fake MCP server");
        let pid = client.child_id();
        let error = client
            .call_tool(
                "too-large",
                json!({"payload": "x".repeat(super::connection::MAX_MCP_RECORD_BYTES)}),
                Duration::from_secs(1),
            )
            .await
            .expect_err("oversized outbound record must fail");
        assert!(matches!(
            error,
            McpError::ProtocolOverflow {
                direction: "outbound",
                resource: "record bytes",
                limit: super::connection::MAX_MCP_RECORD_BYTES,
                ..
            }
        ));
        assert!(
            !process_alive(pid),
            "overflow result preceded cleanup proof"
        );
        assert_reader_joined(&config);
    });
}

#[test]
fn initialize_metadata_is_retained_with_hard_string_and_capability_bounds() {
    let capabilities = (0..40)
        .map(|index| (format!("capability-{index:02}"), json!({})))
        .collect::<serde_json::Map<_, _>>();
    let metadata = super::client::parse_initialize_metadata(json!({
        "protocolVersion": "p".repeat(256),
        "serverInfo": {
            "name": "é".repeat(100),
            "version": "v".repeat(256),
        },
        "capabilities": capabilities,
    }))
    .expect("object initialize result");

    assert_eq!(metadata.protocol_version.unwrap().len(), 128);
    let name = metadata.name.unwrap();
    assert!(name.len() <= 128);
    assert!(name.is_char_boundary(name.len()));
    assert_eq!(metadata.version.unwrap().len(), 128);
    assert_eq!(metadata.capabilities.len(), 32);
}

#[test]
fn codebase_memory_bridge_mcp_initializes_lists_and_calls() {
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = fake_server_script();
    temper_agent_io::block_on(async move {
        let client = connect(fake_command(&dir, None))
            .await
            .expect("connect fake MCP server");
        let metadata = client
            .server_metadata()
            .expect("initialize metadata retained");
        assert_eq!(metadata.protocol_version.as_deref(), Some("2024-11-05"));
        assert_eq!(metadata.name.as_deref(), Some("fake"));
        assert_eq!(metadata.version.as_deref(), Some("1"));
        assert!(metadata.advertises_capability("tools"));
        let tools = client
            .list_tools(Duration::from_secs(1))
            .await
            .expect("list tools");
        assert!(tools.iter().any(|tool| tool.name == "search_code"));
        assert!(tools.iter().any(|tool| tool.name == "delete_project"));

        let output = client
            .call_tool(
                "search_code",
                json!({ "query": "needle" }),
                client.call_timeout(),
            )
            .await
            .expect("call tool");
        assert!(!output.is_error);
        assert!(output.text.contains("called search_code"));
        assert!(output.text.contains("needle"));
    });
}

#[test]
fn codebase_memory_bridge_mcp_startup_timeout_kills_hung_server() {
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = fake_server_script();
    let pid_path = dir.path().join("startup-timeout.pid");
    temper_agent_io::block_on(async move {
        let config =
            fake_command_with_extra(&dir, Some("hang"), Some(pid_path.display().to_string()));
        let error = match connect(config.clone()).await {
            Ok(_) => panic!("hung server times out"),
            Err(error) => error,
        };
        assert!(matches!(error, McpError::Timeout { method, .. } if method == "initialize"));
        let pid = fs::read_to_string(&pid_path)
            .expect("startup server pid")
            .parse()
            .expect("numeric pid");
        assert!(!process_alive(pid), "startup error preceded cleanup proof");
        assert_reader_joined(&config);
    });
}

#[test]
fn codebase_memory_bridge_mcp_child_exits_when_client_drops() {
    let _serial = PROCESS_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let dir = fake_server_script();
    let config = fake_command(&dir, None);
    let connection_config = config.clone();
    let pid = temper_agent_io::block_on(async move {
        let client = connect(connection_config)
            .await
            .expect("connect fake MCP server");
        let pid = client.child_id();
        assert!(process_exists(pid));
        pid
    });

    for _ in 0..50 {
        if !process_exists(pid) {
            assert_reader_joined(&config);
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("MCP child process {pid} still exists after client drop");
}
