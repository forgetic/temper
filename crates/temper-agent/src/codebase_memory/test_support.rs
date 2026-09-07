use super::super::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use temper_protocol_agent::{
    CodebaseMemoryIndex, WorkspaceContext, WorkspaceGuidance, WorkspaceRepository,
    WorkspaceWorkItem,
};

pub(in crate::codebase_memory) fn fake_server_script() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        dir.path().join("fake_codebase_memory_mcp.py"),
        include_str!("tests/fake_codebase_memory_provider.py"),
    )
    .expect("write fake server");
    dir
}

pub(super) fn script_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("fake_codebase_memory_mcp.py")
}

pub(in crate::codebase_memory) fn config(
    dir: &tempfile::TempDir,
    mode: CodebaseMemoryMode,
    index: CodebaseMemoryIndex,
    server_mode: &str,
    log_path: &Path,
    projects: Value,
) -> AgentToolConfig {
    AgentToolConfig {
        codebase_memory: Some(CodebaseMemoryToolConfig {
            mode,
            command: "python3".to_string(),
            args: vec![
                "-u".to_string(),
                script_path(dir).display().to_string(),
                server_mode.to_string(),
                log_path.display().to_string(),
                projects.to_string(),
            ],
            roles: vec!["engineer".to_string()],
            index,
            startup_timeout_secs: 1,
            index_timeout_secs: 1,
            retention: Default::default(),
        }),
    }
}

pub(super) fn bad_command_config(mode: CodebaseMemoryMode) -> AgentToolConfig {
    AgentToolConfig {
        codebase_memory: Some(CodebaseMemoryToolConfig {
            mode,
            command: "definitely-not-a-temper-codebase-memory-mcp".to_string(),
            args: Vec::new(),
            roles: vec!["engineer".to_string()],
            index: CodebaseMemoryIndex::Off,
            startup_timeout_secs: 1,
            index_timeout_secs: 1,
            retention: Default::default(),
        }),
    }
}

pub(in crate::codebase_memory) fn workspace_context(
    cwd: &Path,
    repos: &[(&str, &str, &str)],
) -> WorkspaceContext {
    let repositories = repos
        .iter()
        .enumerate()
        .map(|(index, (owner, name, dir))| {
            fs::create_dir_all(cwd.join(dir)).expect("create repo dir");
            WorkspaceRepository {
                id: format!("repo-{}", index + 1),
                owner: (*owner).to_string(),
                name: (*name).to_string(),
                default_branch: "main".to_string(),
                dir: (*dir).to_string(),
                access: if index == 0 { "writable" } else { "read_only" }.to_string(),
                base_branch: "main".to_string(),
                branch_hint: (index == 0).then(|| "agent/pr-for-code-25".to_string()),
            }
        })
        .collect();
    WorkspaceContext {
        trace_context: None,
        artifact_context: None,
        repos: repositories,
        work_item: WorkspaceWorkItem {
            role: "engineer".to_string(),
            queue: "code_ready".to_string(),
            kind: "code".to_string(),
            target: "Issue { number: ItemNumber(25) }".to_string(),
            context: "{}".to_string(),
        },
        action: "open_pr".to_string(),
        correlation_key: "pr-for-code-25".to_string(),
        checkout: Some("writable".to_string()),
        allowed_verdicts: vec!["needs_architect".to_string()],
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        guidance: WorkspaceGuidance::default(),
        pull_request_freshness: None,
        agent_session: None,
    }
}

pub(super) fn seed_lineage_sources(root: &Path) {
    fs::create_dir_all(root.join("src")).expect("create source directory");
    fs::write(
        root.join("src/lib.rs"),
        "fn run() {} // PRIVATE-SOURCE\nfn caller() {}\nfn source() {}\nfn behavior() {}\n",
    )
    .expect("seed exact lineage source");
}

pub(super) fn output_text(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

pub(super) fn provider_state(log_path: &Path) -> Value {
    let state_path = PathBuf::from(format!("{}.state.json", log_path.display()));
    let raw = fs::read_to_string(&state_path)
        .unwrap_or_else(|error| panic!("read provider state {}: {error}", state_path.display()));
    serde_json::from_str(&raw).expect("provider state is JSON")
}

pub(super) fn tool_calls(log_path: &Path) -> Vec<Value> {
    let Ok(text) = fs::read_to_string(log_path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("tool call log line is json"))
        .collect()
}

pub(super) fn calls_named(log_path: &Path, name: &str) -> Vec<Value> {
    tool_calls(log_path)
        .into_iter()
        .filter(|call| call["name"] == name)
        .collect()
}

pub(super) fn wait_for_calls_named(log_path: &Path, name: &str, count: usize) -> Vec<Value> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let calls = calls_named(log_path, name);
        if calls.len() >= count || std::time::Instant::now() >= deadline {
            return calls;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn process_alive(pid: u32) -> bool {
    let exists = Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !exists {
        return false;
    }
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| stat.rsplit_once(") ").map(|(_, rest)| rest.to_string()))
        .and_then(|rest| rest.chars().next())
        .is_some_and(|state| state != 'Z')
}

pub(super) fn wait_for_process_exit(pid: u32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while process_alive(pid) {
        assert!(
            std::time::Instant::now() < deadline,
            "MCP process {pid} survived circuit-open cleanup"
        );
        std::thread::yield_now();
    }
}
