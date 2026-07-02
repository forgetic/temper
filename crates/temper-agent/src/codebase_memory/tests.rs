use super::*;
use std::fs;
use std::path::PathBuf;

fn fake_server_script() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        dir.path().join("fake_codebase_memory_mcp.py"),
        r#"
import json
import sys
import time

mode = sys.argv[1] if len(sys.argv) > 1 else "normal"
if mode == "hang":
    time.sleep(60)
    sys.exit(0)

TOOLS = [
    {"name": "search_code", "description": "Search indexed code", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}},
    {"name": "get_architecture", "description": "Summarize architecture", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "delete_project", "description": "Delete project", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "manage_adr", "description": "Write ADRs", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "ingest_traces", "description": "Ingest traces", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "query_graph", "description": "Raw graph query", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "index_repository", "description": "Index arbitrary path", "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}}},
]

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

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
        payload = f"{name} result for {json.dumps(args, sort_keys=True)}\n" + ("x" * 20000)
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"content": [{"type": "text", "text": payload}], "isError": False}})
    else:
        send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown method"}})
"#,
    )
        .expect("write fake server");
    dir
}

fn script_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("fake_codebase_memory_mcp.py")
}

fn config(dir: &tempfile::TempDir, mode: CodebaseMemoryMode) -> AgentToolConfig {
    AgentToolConfig {
        codebase_memory: Some(CodebaseMemoryToolConfig {
            mode,
            command: "python3".to_string(),
            args: vec!["-u".to_string(), script_path(dir).display().to_string()],
            roles: vec!["engineer".to_string()],
            index: temper_protocol_agent::CodebaseMemoryIndex::Off,
            startup_timeout_secs: 1,
            index_timeout_secs: 2,
        }),
    }
}

fn hanging_config(dir: &tempfile::TempDir, mode: CodebaseMemoryMode) -> AgentToolConfig {
    let mut config = config(dir, mode);
    let codebase_memory = config
        .codebase_memory
        .as_mut()
        .expect("codebase memory config");
    codebase_memory.args.push("hang".to_string());
    config
}

fn output_text(output: &ToolOutput) -> String {
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

#[test]
fn codebase_memory_bridge_wraps_allowed_tool_and_filters_destructive_tools() {
    let dir = fake_server_script();
    temper_agent_io::block_on(async move {
        let toolset = build_codebase_memory_toolset(
            Some(&config(&dir, CodebaseMemoryMode::Required)),
            "engineer",
        )
        .await
        .expect("build required codebase-memory toolset");
        assert_eq!(toolset.status(), &CodebaseMemoryToolsetStatus::Started);
        let names = toolset.registered_tool_names().to_vec();
        assert!(names.contains(&"codebase_memory_search_code".to_string()));
        assert!(names.contains(&"codebase_memory_get_architecture".to_string()));
        for forbidden in [
            "codebase_memory_delete_project",
            "codebase_memory_manage_adr",
            "codebase_memory_ingest_traces",
            "codebase_memory_query_graph",
            "codebase_memory_index_repository",
        ] {
            assert!(
                !names.contains(&forbidden.to_string()),
                "{forbidden} must not be registered"
            );
        }

        let tools = toolset.into_tools();
        let search = tools
            .iter()
            .find(|tool| tool.name() == "codebase_memory_search_code")
            .expect("search wrapper present");
        assert_eq!(search.effects(), ToolEffects::read());
        let output = search
            .execute("call-1", json!({ "query": "needle" }), None)
            .await
            .expect("execute wrapped MCP tool");
        let text = output_text(&output);
        assert!(!output.is_error);
        assert!(text.contains("search_code result"));
        assert!(text.contains("needle"));
        assert!(text.contains("output truncated"));
        assert!(text.len() <= MAX_CODEBASE_MEMORY_OUTPUT_BYTES);
    });
}

#[test]
fn codebase_memory_bridge_auto_vs_required_startup_failures() {
    let auto = AgentToolConfig {
        codebase_memory: Some(CodebaseMemoryToolConfig {
            mode: CodebaseMemoryMode::Auto,
            command: "definitely-not-a-temper-codebase-memory-mcp".to_string(),
            args: Vec::new(),
            roles: vec!["engineer".to_string()],
            index: temper_protocol_agent::CodebaseMemoryIndex::Off,
            startup_timeout_secs: 1,
            index_timeout_secs: 1,
        }),
    };
    let required = AgentToolConfig {
        codebase_memory: Some(CodebaseMemoryToolConfig {
            mode: CodebaseMemoryMode::Required,
            command: "definitely-not-a-temper-codebase-memory-mcp".to_string(),
            args: Vec::new(),
            roles: vec!["engineer".to_string()],
            index: temper_protocol_agent::CodebaseMemoryIndex::Off,
            startup_timeout_secs: 1,
            index_timeout_secs: 1,
        }),
    };

    temper_agent_io::block_on(async move {
        let auto_toolset = build_codebase_memory_toolset(Some(&auto), "engineer")
            .await
            .expect("auto mode suppresses startup failure");
        assert!(matches!(
            auto_toolset.status(),
            CodebaseMemoryToolsetStatus::AutoUnavailable { reason }
                if reason.contains("spawn MCP command")
        ));
        assert!(auto_toolset.registered_tool_names().is_empty());

        let required_error = match build_codebase_memory_toolset(Some(&required), "engineer").await
        {
            Ok(_) => panic!("required mode hard-fails startup failure"),
            Err(error) => error,
        };
        assert!(
            required_error
                .to_string()
                .contains("required codebase-memory MCP startup failed")
        );
    });
}

#[test]
fn codebase_memory_bridge_auto_timeout_is_best_effort_required_timeout_fails() {
    let dir = fake_server_script();
    temper_agent_io::block_on(async move {
        let auto = hanging_config(&dir, CodebaseMemoryMode::Auto);
        let auto_toolset = build_codebase_memory_toolset(Some(&auto), "engineer")
            .await
            .expect("auto mode suppresses timeout");
        assert!(matches!(
            auto_toolset.status(),
            CodebaseMemoryToolsetStatus::AutoUnavailable { reason }
                if reason.contains("timed out")
        ));

        let required = hanging_config(&dir, CodebaseMemoryMode::Required);
        let error = match build_codebase_memory_toolset(Some(&required), "engineer").await {
            Ok(_) => panic!("required mode fails timeout"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("timed out"));
    });
}
