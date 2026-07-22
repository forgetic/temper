use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jig_core::{Reply, Script, StopReason, Turn};
use jig_server::FakeLlm;
use temper_agent::{
    ProviderConfig, WorkspaceContext, WorkspaceGuidance, WorkspaceRepository, WorkspaceWorkItem,
    run_coding_agent_native_with_tool_config,
};
use temper_protocol_agent::{
    AgentToolConfig, CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig,
};

#[allow(dead_code)]
#[path = "support/coding_agent_workspace.rs"]
mod coding_agent_workspace;
use coding_agent_workspace::{REPO_DIR, TempCheckout};

const FAKE_MCP_DESCRIPTION_SENTINEL: &str = "FAKE-MCP-DESCRIPTION-SENTINEL-384";

#[test]
fn jig_coding_agent_can_call_registered_codebase_memory_tool() {
    let checkout = TempCheckout::new("jig-codebase-memory-tool-call");
    checkout.init_git();

    let observed_memory_result = Arc::new(AtomicUsize::new(0));
    let fake = codebase_memory_agent_fake(Arc::clone(&observed_memory_result));
    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-codebase-memory-tool-call",
        "https://example.invalid/unused-production-url",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url());
    let mcp_dir = fake_codebase_memory_mcp_script();
    let tool_config = codebase_memory_tool_config(&mcp_dir);

    let context = workspace_context();
    let cwd = checkout.path().to_path_buf();
    let result = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_coding_agent_native_with_tool_config(
            handle,
            &provider,
            &context,
            &cwd,
            8,
            None,
            Some(&tool_config),
        )
        .await
    })
    .expect("native jig-backed coding agent can use codebase-memory tool");

    assert_eq!(result.verdict, None);
    assert!(
        result
            .summary
            .as_deref()
            .unwrap_or_default()
            .contains("codebase memory")
    );
    assert_eq!(
        fs::read_to_string(checkout.repo_path().join("MEMORY_NOTES.md"))
            .expect("MEMORY_NOTES.md was written"),
        "memory-guided notes\n"
    );
    assert_eq!(observed_memory_result.load(Ordering::SeqCst), 1);
    let requests = fake.requests();
    let first_request = requests.first().expect("fake LLM saw first request");
    let first_view = first_request
        .view
        .as_ref()
        .expect("fake LLM first request was normalized");
    let prompt = first_view
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        prompt.contains("CODEBASE MEMORY"),
        "first request should include prompt guidance only because tools registered"
    );
    assert!(prompt.contains("Use them early for non-trivial tasks"));
    for duplicated_api_text in [
        FAKE_MCP_DESCRIPTION_SENTINEL,
        "codebase_memory_search_code",
        "Registered codebase-memory tools:",
    ] {
        assert!(
            !prompt.contains(duplicated_api_text),
            "provider prompt duplicated tool API text {duplicated_api_text:?}"
        );
    }

    let request_json: serde_json::Value =
        serde_json::from_slice(&first_request.body).expect("provider request JSON");
    let memory_tools = request_json["tools"]
        .as_array()
        .expect("provider tools array")
        .iter()
        .filter(|tool| {
            tool.get("name")
                .or_else(|| tool.pointer("/function/name"))
                .and_then(serde_json::Value::as_str)
                == Some("codebase_memory_search_code")
        })
        .collect::<Vec<_>>();
    assert_eq!(memory_tools.len(), 1, "memory tool registered exactly once");
    let expected_description = format!(
        "{FAKE_MCP_DESCRIPTION_SENTINEL}\n\n\
         Workspace scoped: default project `acme/demo`; accepted `project`/`repo` aliases: acme/demo, demo, repo-1. Unknown aliases and filesystem paths are rejected.\n\n\
         Read-only wrapper around codebase-memory MCP tool `search_code`."
    );
    let provider_description = memory_tools[0]
        .get("description")
        .or_else(|| memory_tools[0].pointer("/function/description"))
        .and_then(serde_json::Value::as_str);
    assert_eq!(
        provider_description,
        Some(expected_description.as_str()),
        "provider retains the complete wrapped MCP description"
    );
    assert_eq!(
        String::from_utf8_lossy(&first_request.body)
            .matches(FAKE_MCP_DESCRIPTION_SENTINEL)
            .count(),
        1,
        "fake MCP description appears exactly once in the provider tool definition"
    );
}

fn codebase_memory_agent_fake(observed_memory_result: Arc<AtomicUsize>) -> FakeLlm {
    FakeLlm::start(Script::rule(move |view| match view.prior_tool_results {
        0 => Reply {
            turns: vec![Turn::ToolCall {
                id: "call_memory_search".to_string(),
                name: "codebase_memory_search_code".to_string(),
                args: serde_json::json!({ "query": "WidgetService" }),
            }],
            usage: Default::default(),
            stop: StopReason::ToolCalls,
        },
        1 => {
            let saw_memory_result = view.messages.iter().any(|message| {
                message.role == "tool" && message.content.contains("FAKE_MCP_SEARCH_RESULT")
            });
            assert!(
                saw_memory_result,
                "fake LLM did not receive the codebase-memory MCP result"
            );
            observed_memory_result.fetch_add(1, Ordering::SeqCst);
            Reply {
                turns: vec![Turn::ToolCall {
                    id: "call_write_memory_notes".to_string(),
                    name: "write".to_string(),
                    args: serde_json::json!({
                        "path": "demo/MEMORY_NOTES.md",
                        "content": "memory-guided notes\n"
                    }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            }
        }
        _ => Reply::text(
            r#"{"summary":"Used codebase memory search result before writing MEMORY_NOTES.md."}"#,
        ),
    }))
    .expect("start codebase-memory fake LLM")
}

fn fake_codebase_memory_mcp_script() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        dir.path().join("fake_codebase_memory_mcp.py"),
        r#"
import json
import sys

TOOLS = [
    {"name": "search_code", "description": "FAKE-MCP-DESCRIPTION-SENTINEL-384", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}},
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
        args = params.get("arguments") or {}
        query = args.get("query", "")
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"content": [{"type": "text", "text": "FAKE_MCP_SEARCH_RESULT for " + query}], "isError": False}})
    else:
        send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown method"}})
"#,
    )
    .expect("write fake codebase-memory MCP server");
    dir
}

fn codebase_memory_tool_config(dir: &tempfile::TempDir) -> AgentToolConfig {
    AgentToolConfig {
        codebase_memory: Some(CodebaseMemoryToolConfig {
            mode: CodebaseMemoryMode::Required,
            command: "python3".to_string(),
            args: vec![
                "-u".to_string(),
                fake_codebase_memory_mcp_path(dir).display().to_string(),
            ],
            roles: vec!["engineer".to_string()],
            index: CodebaseMemoryIndex::Off,
            startup_timeout_secs: 1,
            index_timeout_secs: 2,
        }),
    }
}

fn fake_codebase_memory_mcp_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("fake_codebase_memory_mcp.py")
}

fn workspace_context() -> WorkspaceContext {
    WorkspaceContext {
        trace_context: None,
        artifact_context: None,
        repos: vec![WorkspaceRepository {
            id: "repo-1".to_string(),
            owner: "acme".to_string(),
            name: "demo".to_string(),
            default_branch: "main".to_string(),
            dir: REPO_DIR.to_string(),
            access: "writable".to_string(),
            base_branch: "main".to_string(),
            branch_hint: Some("agent/pr-for-code-25".to_string()),
        }],
        work_item: WorkspaceWorkItem {
            role: "engineer".to_string(),
            queue: "code_ready".to_string(),
            kind: "code".to_string(),
            target: "Issue { number: ItemNumber(25) }".to_string(),
            context: serde_json::json!({
                "artifact": {
                    "type": "issue",
                    "number": 25,
                    "title": "Create deterministic notes",
                    "body": "Create NOTES.md whose first line is exactly `project notes`.",
                    "labels": ["code", "ready"],
                    "state": "Open"
                }
            })
            .to_string(),
        },
        action: "open_pr".to_string(),
        correlation_key: "pr-for-code-25".to_string(),
        checkout: Some("writable".to_string()),
        allowed_verdicts: vec!["needs_architect".to_string()],
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        guidance: WorkspaceGuidance {
            role_guidance: Some(
                "Make a real product diff by creating NOTES.md. Do not create .temper-only bookkeeping diffs."
                    .to_string(),
            ),
            tool_guidance: Some("Use the available workspace tools to edit files.".to_string()),
            tool_constraints: vec!["Do not run git commit.".to_string()],
            action_guidance: None,
        },
        pull_request_freshness: None,
        agent_session: None,
    }
}
