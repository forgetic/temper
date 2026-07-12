use super::super::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use temper_protocol_agent::{
    CodebaseMemoryIndex, WorkspaceContext, WorkspaceGuidance, WorkspaceRepository,
    WorkspaceWorkItem,
};

pub(super) fn fake_server_script() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        dir.path().join("fake_codebase_memory_mcp.py"),
        r#"
import json
import os
import sys
import time

mode = sys.argv[1] if len(sys.argv) > 1 else "normal"
log_path = sys.argv[2] if len(sys.argv) > 2 else ""
projects_arg = sys.argv[3] if len(sys.argv) > 3 else '{"projects": []}'
if mode == "hang":
    time.sleep(60)
    sys.exit(0)

try:
    parsed = json.loads(projects_arg)
    projects = parsed.get("projects", parsed if isinstance(parsed, list) else [])
except Exception:
    projects = []

state_path = log_path + ".projects.json" if log_path else ""

def current_projects():
    if mode.startswith("index-rediscovers") and state_path and os.path.exists(state_path):
        with open(state_path, "r", encoding="utf-8") as handle:
            parsed = json.load(handle)
        return parsed.get("projects", parsed if isinstance(parsed, list) else [])
    return projects

TOOLS = [
    {"name": "search_code", "description": "Search indexed code", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "project": {"type": "string"}}, "required": ["query"]}},
    {"name": "get_architecture", "description": "Summarize architecture", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}},
    {"name": "list_projects", "description": "List projects", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "index_status", "description": "Index status", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}},
    {"name": "detect_changes", "description": "Detect changes", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}}},
    {"name": "delete_project", "description": "Delete project", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "manage_adr", "description": "Write ADRs", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "ingest_traces", "description": "Ingest traces", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "query_graph", "description": "Raw graph query", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "index_repository", "description": "Index arbitrary path", "inputSchema": {"type": "object", "properties": {"repo_path": {"type": "string"}}}},
]

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

def log_tool(name, args):
    if not log_path:
        return
    with open(log_path, "a", encoding="utf-8") as handle:
        handle.write(json.dumps({"name": name, "arguments": args}, sort_keys=True) + "\n")

for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"protocolVersion": "2024-11-05", "serverInfo": {"name": "fake-codebase-memory", "version": "1"}, "capabilities": {"tools": {}}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"tools": TOOLS}})
    elif method == "tools/call":
        params = request.get("params", {})
        name = params.get("name")
        args = params.get("arguments") or {}
        log_tool(name, args)
        if name == "list_projects":
            payload = json.dumps({"projects": current_projects()}, sort_keys=True)
        elif name == "index_repository":
            repo_path = args.get("repo_path", "")
            if not isinstance(repo_path, str) or not repo_path:
                payload = "index_repository requires repo_path"
                send({"jsonrpc": "2.0", "id": request["id"], "result": {"content": [{"type": "text", "text": payload}], "isError": True}})
                continue
            if mode == "index-hang":
                time.sleep(60)
            if mode == "index-rediscovers-slow":
                time.sleep(0.15)
            if mode.startswith("index-rediscovers") and state_path:
                with open(state_path, "w", encoding="utf-8") as handle:
                    json.dump({"projects": [{"name": "generated-project", "root_path": repo_path}]}, handle)
                payload = json.dumps({"project": {"root_path": repo_path}}, sort_keys=True)
            else:
                payload = json.dumps({"project": {"id": "indexed:" + repo_path, "name": "indexed-" + os.path.basename(repo_path), "repo_path": repo_path}}, sort_keys=True)
        else:
            payload = f"{name} result for {json.dumps(args, sort_keys=True)}\n" + ("x" * 20000)
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"content": [{"type": "text", "text": payload}], "isError": False}})
    else:
        send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown method"}})
"#,
    )
    .expect("write fake server");
    dir
}

pub(super) fn script_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("fake_codebase_memory_mcp.py")
}

pub(super) fn config(
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
        }),
    }
}

pub(super) fn workspace_context(cwd: &Path, repos: &[(&str, &str, &str)]) -> WorkspaceContext {
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
