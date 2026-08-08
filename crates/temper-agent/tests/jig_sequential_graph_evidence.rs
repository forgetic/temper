use std::fs;
use std::sync::{Arc, Mutex};

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

const IMPLEMENTATION: &str = "RESULT_IMPLEMENTATION";
const REFINEMENT: &str = "RESULT_REFINEMENT";
const CALLER_OR_MODEL: &str = "RESULT_CALLER_OR_MODEL";
const IMPLEMENTATION_SOURCE: &str = "RESULT_IMPLEMENTATION_SOURCE";
const BEHAVIORAL_TEST: &str = "RESULT_FOCUSED_BEHAVIORAL_TEST";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DecisionStep {
    Discovery,
    Refinement,
    Trace,
    ImplementationSource,
    BehavioralTestSource,
    Mutation,
    Complete,
}

#[test]
fn jig_agent_consumes_sequential_graph_evidence_before_mutation() {
    let checkout = TempCheckout::new("jig-sequential-graph-evidence");
    checkout.init_git();

    let observed_steps = Arc::new(Mutex::new(Vec::new()));
    let fake = sequential_evidence_fake(Arc::clone(&observed_steps));
    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-sequential-graph-evidence",
        "https://example.invalid/unused-production-url",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url());
    let mcp_dir = sequential_evidence_mcp();
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
    .expect("native Jig agent consumes the graph decision chain");

    assert_eq!(result.verdict, None);
    assert_eq!(
        fs::read_to_string(checkout.repo_path().join("EVIDENCE.md"))
            .expect("mutation follows the completed evidence chain"),
        "sequential graph evidence\n"
    );
    assert_eq!(
        *observed_steps.lock().expect("decision steps lock"),
        vec![
            DecisionStep::Discovery,
            DecisionStep::Refinement,
            DecisionStep::Trace,
            DecisionStep::ImplementationSource,
            DecisionStep::BehavioralTestSource,
            DecisionStep::Mutation,
            DecisionStep::Complete,
        ],
        "dependent producer/consumer targets must be selected in successive model turns"
    );
}

fn sequential_evidence_fake(observed_steps: Arc<Mutex<Vec<DecisionStep>>>) -> FakeLlm {
    FakeLlm::start(Script::rule(move |view| {
        let consumed = |evidence| {
            view.messages.iter().any(|message| {
                message.role == "tool" && message.content.contains(evidence)
            })
        };
        let record = |step| observed_steps.lock().expect("decision steps lock").push(step);

        match view.prior_tool_results {
            0 => {
                record(DecisionStep::Discovery);
                tool_reply(
                    "discover-implementation",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "implementation"}),
                )
            }
            1 => {
                assert!(
                    consumed(IMPLEMENTATION),
                    "refinement requires the preceding successful implementation result"
                );
                record(DecisionStep::Refinement);
                tool_reply(
                    "refine-implementation",
                    "codebase_memory_search_code",
                    serde_json::json!({"pattern": "refinement"}),
                )
            }
            2 => {
                assert!(
                    consumed(REFINEMENT),
                    "trace requires the preceding successful refinement result"
                );
                record(DecisionStep::Trace);
                tool_reply(
                    "trace-caller-or-model",
                    "codebase_memory_trace_path",
                    serde_json::json!({"function_name": "refinement"}),
                )
            }
            3 => {
                assert!(
                    consumed(CALLER_OR_MODEL),
                    "implementation source read requires the preceding successful trace result"
                );
                record(DecisionStep::ImplementationSource);
                tool_reply(
                    "read-implementation",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({"qualified_name": "implementation"}),
                )
            }
            4 => {
                assert!(
                    consumed(IMPLEMENTATION_SOURCE),
                    "behavioral test read requires the implementation source result"
                );
                record(DecisionStep::BehavioralTestSource);
                tool_reply(
                    "read-behavioral-test",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({"qualified_name": "behavioral_test"}),
                )
            }
            5 => {
                assert!(
                    consumed(IMPLEMENTATION_SOURCE)
                        && consumed(CALLER_OR_MODEL)
                        && consumed(BEHAVIORAL_TEST),
                    "mutation requires implementation, caller-or-model, and focused behavioral-test evidence"
                );
                record(DecisionStep::Mutation);
                tool_reply(
                    "mutate-after-evidence",
                    "write",
                    serde_json::json!({
                        "path": "demo/EVIDENCE.md",
                        "content": "sequential graph evidence\n"
                    }),
                )
            }
            6 => {
                record(DecisionStep::Complete);
                Reply::text(r#"{"summary":"Mutated after sequential graph evidence."}"#)
            }
            turn => panic!("unexpected model turn {turn}"),
        }
    }))
    .expect("start sequential-evidence fake LLM")
}

fn tool_reply(id: &str, name: &str, args: serde_json::Value) -> Reply {
    Reply {
        turns: vec![Turn::ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            args,
        }],
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }
}

fn sequential_evidence_mcp() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("MCP tempdir");
    fs::write(
        dir.path().join("sequential_evidence_mcp.py"),
        r#"
import json
import sys

TOOLS = [
    {"name": "search_graph", "description": "Targeted graph search", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "project": {"type": "string"}}, "required": ["query"]}},
    {"name": "search_code", "description": "Targeted code search", "inputSchema": {"type": "object", "properties": {"pattern": {"type": "string"}, "project": {"type": "string"}}, "required": ["pattern"]}},
    {"name": "trace_path", "description": "Targeted caller trace", "inputSchema": {"type": "object", "properties": {"function_name": {"type": "string"}, "project": {"type": "string"}}, "required": ["function_name"]}},
    {"name": "get_code_snippet", "description": "Targeted source read", "inputSchema": {"type": "object", "properties": {"qualified_name": {"type": "string"}, "project": {"type": "string"}}, "required": ["qualified_name"]}},
    {"name": "index_status", "description": "Index status", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}, "required": ["project"]}},
    {"name": "index_repository", "description": "Stable repository upsert", "inputSchema": {"type": "object", "properties": {"repo_path": {"type": "string"}, "name": {"type": "string"}}, "required": ["repo_path", "name"]}},
]

snippet_results = iter(["RESULT_IMPLEMENTATION_SOURCE", "RESULT_FOCUSED_BEHAVIORAL_TEST"])
results = {
    "search_graph": "RESULT_IMPLEMENTATION",
    "search_code": "RESULT_REFINEMENT",
    "trace_path": "RESULT_CALLER_OR_MODEL",
}

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

def rpc_result(request_id, payload):
    send({"jsonrpc": "2.0", "id": request_id, "result": payload})

def tool_result(request_id, payload):
    rpc_result(request_id, {"content": [{"type": "text", "text": payload}], "isError": False})

for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    if "id" not in request:
        continue
    if request.get("method") == "initialize":
        rpc_result(request["id"], {"protocolVersion": "2024-11-05", "serverInfo": {"name": "codebase-memory-mcp", "version": "0.9.0"}, "capabilities": {"tools": {}}})
    elif request.get("method") == "tools/list":
        rpc_result(request["id"], {"tools": TOOLS})
    elif request.get("method") == "tools/call":
        name = request.get("params", {}).get("name")
        if name == "index_status":
            payload = {"status": "fresh"}
        elif name == "get_code_snippet":
            payload = {"results": [{"evidence": next(snippet_results)}]}
        else:
            payload = {"results": [{"evidence": results.get(name, "RESULT_UNKNOWN")}]}
        tool_result(request["id"], json.dumps(payload))
    else:
        send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown method"}})
"#,
    )
    .expect("write sequential-evidence MCP server");
    dir
}

fn codebase_memory_tool_config(dir: &tempfile::TempDir) -> AgentToolConfig {
    AgentToolConfig {
        codebase_memory: Some(CodebaseMemoryToolConfig {
            mode: CodebaseMemoryMode::Required,
            command: "python3".to_string(),
            args: vec![
                "-u".to_string(),
                dir.path()
                    .join("sequential_evidence_mcp.py")
                    .display()
                    .to_string(),
            ],
            roles: vec!["engineer".to_string()],
            index: CodebaseMemoryIndex::Off,
            startup_timeout_secs: 1,
            index_timeout_secs: 2,
            retention: Default::default(),
        }),
    }
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
            branch_hint: Some("agent/pr-for-code-976".to_string()),
        }],
        work_item: WorkspaceWorkItem {
            role: "engineer".to_string(),
            queue: "code_ready".to_string(),
            kind: "code".to_string(),
            target: "Issue { number: ItemNumber(976) }".to_string(),
            context: "{}".to_string(),
        },
        action: "open_pr".to_string(),
        correlation_key: "pr-for-code-976".to_string(),
        checkout: Some("writable".to_string()),
        allowed_verdicts: vec!["needs_architect".to_string()],
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        guidance: WorkspaceGuidance::default(),
        pull_request_freshness: None,
        agent_session: None,
    }
}
