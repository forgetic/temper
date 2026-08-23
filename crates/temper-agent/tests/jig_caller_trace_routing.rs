use std::fs;
use std::path::Path;
use std::sync::Mutex;

use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::Value as JsonValue;
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

const IMPLEMENTATION: &str = "crate::routing::select_worker";
const CALLER: &str = "crate::delivery::dispatch";
const FOCUSED_TEST: &str = "crate::tests::keeps_affinity";
static JIG_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn jig_routes_empty_inbound_trace_and_exact_source_relationships() {
    let _serial = JIG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let checkout = TempCheckout::new("jig-caller-trace-implementation-root");
    checkout.init_git();
    let mcp = fake_mcp();
    let fake = FakeLlm::start(Script::rule(minimal_routing_reply)).expect("start routing fake LLM");
    let provider = provider(&fake, "jig-caller-trace-implementation-root");
    let config = tool_config(&mcp);
    let context = workspace_context();
    let cwd = checkout.path().to_path_buf();

    let result = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_coding_agent_native_with_tool_config(
            handle,
            &provider,
            &context,
            &cwd,
            9,
            None,
            Some(&config),
        )
        .await
    })
    .expect("provider-selected caller route completes");

    assert_eq!(result.verdict, None);
    assert_eq!(
        fs::read_to_string(checkout.repo_path().join("CALLER_ROUTE.md")).expect("routing product"),
        "caller route verified\n"
    );
    let calls = graph_calls(mcp.path());
    assert_eq!(
        tool_names(&calls),
        [
            "search_code",
            "trace_path",
            "get_code_snippet",
            "get_code_snippet",
            "get_code_snippet",
        ]
    );
    assert_eq!(calls[1]["arguments"]["function_name"], IMPLEMENTATION);
    assert_eq!(calls[1]["arguments"]["direction"], "inbound");
    assert_eq!(calls[2]["arguments"]["qualified_name"], IMPLEMENTATION);
    assert_eq!(calls[3]["arguments"]["qualified_name"], CALLER);
    assert_eq!(calls[4]["arguments"]["qualified_name"], FOCUSED_TEST);
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["name"] == "get_code_snippet"
                && call["arguments"]["qualified_name"] == IMPLEMENTATION)
            .count(),
        1,
        "an empty inbound trace must not cause same-symbol caller reads"
    );
}

#[test]
fn jig_does_not_repeat_a_locally_denied_selector_evidence_pair() {
    let _serial = JIG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let checkout = TempCheckout::new("jig-caller-trace-denial-recovery");
    checkout.init_git();
    let mcp = fake_mcp();
    let fake = FakeLlm::start(Script::rule(recovery_routing_reply))
        .expect("start recovery routing fake LLM");
    let provider = provider(&fake, "jig-caller-trace-denial-recovery");
    let config = tool_config(&mcp);
    let context = workspace_context();
    let cwd = checkout.path().to_path_buf();

    let result = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_coding_agent_native_with_tool_config(
            handle,
            &provider,
            &context,
            &cwd,
            12,
            None,
            Some(&config),
        )
        .await
    })
    .expect("compatible recovery menu completes");

    assert_eq!(result.verdict, None);
    assert_eq!(
        fs::read_to_string(checkout.repo_path().join("RECOVERY_ROUTE.md"))
            .expect("recovery product"),
        "denied trace was not repeated\n"
    );
    let calls = graph_calls(mcp.path());
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["name"] == "trace_path")
            .count(),
        1,
        "the locally denied duplicate trace must not reach the provider or be retried"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["name"] == "get_code_snippet"
                && call["arguments"]["qualified_name"] == IMPLEMENTATION)
            .count(),
        1,
        "recovery must use returned relationship identities instead of rereading the implementation"
    );
}

fn minimal_routing_reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0 => search_reply("discover-likely-implementation"),
        1 => trace_reply("trace-likely-implementation", implementation_target(view)),
        2 => {
            assert_empty_inbound_trace(view);
            source_reply(
                "read-selected-implementation",
                implementation_target(view),
                "implementation",
            )
        }
        3 => {
            let (caller, _) = exact_source_relationships(view);
            source_reply("read-returned-caller", caller, "caller")
        }
        4 => {
            let (_, test) = exact_source_relationships(view);
            source_reply("read-returned-focused-test", test, "focused_test")
        }
        5 => tool_reply(
            "write-after-routed-evidence",
            "write",
            serde_json::json!({
                "path": "demo/CALLER_ROUTE.md",
                "content": "caller route verified\n"
            }),
        ),
        6 => Reply::text(
            r#"{"summary":"Selected the implementation root and consumed returned caller evidence."}"#,
        ),
        count => panic!("unexpected minimal routing tool-result count {count}"),
    }
}

fn recovery_routing_reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0 => search_reply("discover-recovery-implementation"),
        1 => trace_reply("trace-recovery-implementation", implementation_target(view)),
        2 => {
            assert_empty_inbound_trace(view);
            source_reply(
                "read-recovery-implementation",
                implementation_target(view),
                "implementation",
            )
        }
        3 => refinement_reply(
            "non-progressing-refinement-one",
            implementation_target(view),
        ),
        4 => refinement_reply(
            "non-progressing-refinement-two",
            implementation_target(view),
        ),
        5 => {
            assert!(messages_contain(
                view,
                "decision-evidence recovery required"
            ));
            trace_reply("denied-satisfied-trace", implementation_target(view))
        }
        6 => {
            assert!(messages_contain(
                view,
                "decision-evidence recovery required"
            ));
            assert!(messages_contain(
                view,
                "missing evidence: [caller, focused_test]"
            ));
            let (caller, test) = exact_source_relationships(view);
            tool_batch(&[
                (
                    "recover-returned-caller",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": caller,
                        "decision_evidence_kind": "caller"
                    }),
                ),
                (
                    "recover-returned-focused-test",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": test,
                        "decision_evidence_kind": "focused_test"
                    }),
                ),
            ])
        }
        8 => tool_reply(
            "write-after-compatible-recovery",
            "write",
            serde_json::json!({
                "path": "demo/RECOVERY_ROUTE.md",
                "content": "denied trace was not repeated\n"
            }),
        ),
        9 => Reply::text(
            r#"{"summary":"Followed the compatible menu without repeating the denied trace."}"#,
        ),
        count => panic!("unexpected recovery routing tool-result count {count}"),
    }
}

fn search_reply(id: &str) -> Reply {
    tool_reply(
        id,
        "codebase_memory_search_code",
        serde_json::json!({"query": "worker selection", "pattern": "select worker"}),
    )
}

fn trace_reply(id: &str, function_name: String) -> Reply {
    tool_reply(
        id,
        "codebase_memory_trace_path",
        serde_json::json!({"function_name": function_name, "direction": "inbound"}),
    )
}

fn refinement_reply(id: &str, pattern: String) -> Reply {
    tool_reply(
        id,
        "codebase_memory_search_code",
        serde_json::json!({"query": "worker selection", "pattern": pattern}),
    )
}

fn source_reply(id: &str, qualified_name: String, kind: &str) -> Reply {
    tool_reply(
        id,
        "codebase_memory_get_code_snippet",
        serde_json::json!({
            "qualified_name": qualified_name,
            "decision_evidence_kind": kind
        }),
    )
}

fn tool_reply(id: &str, name: &str, args: JsonValue) -> Reply {
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

fn tool_batch(calls: &[(&str, &str, JsonValue)]) -> Reply {
    Reply {
        turns: calls
            .iter()
            .map(|(id, name, args)| Turn::ToolCall {
                id: (*id).to_string(),
                name: (*name).to_string(),
                args: args.clone(),
            })
            .collect(),
        usage: Default::default(),
        stop: StopReason::ToolCalls,
    }
}

fn implementation_target(view: &RequestView) -> String {
    provider_results(view)
        .iter()
        .find_map(|result| result.pointer("/results/0/qualified_name"))
        .and_then(JsonValue::as_str)
        .map(str::to_string)
        .expect("targeted search returned a likely implementation")
}

fn exact_source_relationships(view: &RequestView) -> (String, String) {
    let source = provider_results(view)
        .into_iter()
        .find(|result| result.get("source").is_some() && result.get("callers").is_some())
        .expect("exact implementation source returned typed relationships");
    let caller = source
        .pointer("/callers/0/qualified_name")
        .and_then(JsonValue::as_str)
        .expect("exact source returned a caller");
    let test = source
        .pointer("/related_source_references/0/qualified_name")
        .and_then(JsonValue::as_str)
        .expect("exact source returned a focused test");
    (caller.to_string(), test.to_string())
}

fn assert_empty_inbound_trace(view: &RequestView) {
    let trace = provider_results(view)
        .into_iter()
        .find(|result| result.get("direction").is_some())
        .expect("inbound trace result");
    assert_eq!(trace["direction"], "inbound");
    assert_eq!(trace["complete"], true);
    assert_eq!(trace["callers"], serde_json::json!([]));
}

fn provider_results(view: &RequestView) -> Vec<JsonValue> {
    view.messages
        .iter()
        .filter(|message| message.role == "tool")
        .filter_map(|message| {
            let content = message
                .content
                .split_once("\n\n[Decision anchor:")
                .map_or(message.content.as_str(), |(result, _)| result);
            serde_json::from_str(content).ok()
        })
        .collect()
}

fn messages_contain(view: &RequestView, needle: &str) -> bool {
    view.messages
        .iter()
        .any(|message| message.content.contains(needle))
}

fn graph_calls(dir: &Path) -> Vec<JsonValue> {
    fs::read_to_string(dir.join("calls.jsonl"))
        .expect("MCP call log")
        .lines()
        .map(|line| serde_json::from_str::<JsonValue>(line).expect("MCP call record"))
        .filter(|call| call["name"] != "index_status")
        .collect()
}

fn tool_names(calls: &[JsonValue]) -> Vec<&str> {
    calls
        .iter()
        .map(|call| call["name"].as_str().expect("tool name"))
        .collect()
}

fn provider(fake: &FakeLlm, model: &str) -> ProviderConfig {
    ProviderConfig::new(
        "jig-openai-compatible",
        model,
        "https://example.invalid/unused-production-url",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url())
}

fn fake_mcp() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("MCP tempdir");
    fs::write(dir.path().join("fake.py"), FAKE_MCP).expect("write fake MCP");
    dir
}

fn tool_config(dir: &tempfile::TempDir) -> AgentToolConfig {
    AgentToolConfig {
        codebase_memory: Some(CodebaseMemoryToolConfig {
            mode: CodebaseMemoryMode::Required,
            command: "python3".to_string(),
            args: vec![
                "-u".to_string(),
                dir.path().join("fake.py").display().to_string(),
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
            branch_hint: Some("agent/pr-for-code-25".to_string()),
        }],
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

const FAKE_MCP: &str = r#"
import json
import os
import sys

IMPLEMENTATION = "crate::routing::select_worker"
CALLER = "crate::delivery::dispatch"
FOCUSED_TEST = "crate::tests::keeps_affinity"
CALL_LOG = os.path.join(os.path.dirname(__file__), "calls.jsonl")

TOOLS = [
    {"name": "search_code", "description": "Targeted code search", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "pattern": {"type": "string"}, "project": {"type": "string"}}, "required": ["query"]}},
    {"name": "trace_path", "description": "Targeted caller trace", "inputSchema": {"type": "object", "properties": {"function_name": {"type": "string"}, "direction": {"type": "string"}, "project": {"type": "string"}}, "required": ["function_name"]}},
    {"name": "get_code_snippet", "description": "Targeted source read", "inputSchema": {"type": "object", "properties": {"qualified_name": {"type": "string"}, "project": {"type": "string"}}, "required": ["qualified_name"]}},
    {"name": "index_status", "description": "Index status", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}, "required": ["project"]}},
    {"name": "index_repository", "description": "Index repository", "inputSchema": {"type": "object", "properties": {"repo_path": {"type": "string"}, "name": {"type": "string"}}, "required": ["repo_path", "name"]}},
]

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

def result(request_id, payload):
    send({"jsonrpc": "2.0", "id": request_id, "result": {"content": [{"type": "text", "text": json.dumps(payload)}], "isError": False}})

for line in sys.stdin:
    if not line.strip():
        continue
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"protocolVersion": "2024-11-05", "serverInfo": {"name": "codebase-memory-mcp", "version": "0.9.0"}, "capabilities": {"tools": {}}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": request["id"], "result": {"tools": TOOLS}})
    elif method == "tools/call":
        params = request.get("params") or {}
        name = params.get("name", "")
        arguments = params.get("arguments") or {}
        with open(CALL_LOG, "a", encoding="utf-8") as log:
            log.write(json.dumps({"name": name, "arguments": arguments}, sort_keys=True) + "\n")
        if name == "index_status":
            result(request["id"], {"project": arguments.get("project", ""), "status": "fresh"})
        elif name == "search_code":
            result(request["id"], {"results": [{"qualified_name": IMPLEMENTATION, "name": "select_worker"}]})
        elif name == "trace_path":
            assert arguments.get("function_name") == IMPLEMENTATION
            assert arguments.get("direction") == "inbound"
            result(request["id"], {"function": {"qualified_name": IMPLEMENTATION, "name": "select_worker"}, "direction": "inbound", "complete": True, "callers": []})
        elif name == "get_code_snippet":
            selected = arguments.get("qualified_name")
            if selected == IMPLEMENTATION:
                result(request["id"], {"qualified_name": IMPLEMENTATION, "name": "select_worker", "source": "implementation source", "callers": [{"qualified_name": CALLER, "name": "dispatch"}], "related_source_references": [{"qualified_name": FOCUSED_TEST, "name": "keeps_affinity"}]})
            elif selected == CALLER:
                result(request["id"], {"qualified_name": CALLER, "name": "dispatch", "source": "caller source", "callees": [{"qualified_name": IMPLEMENTATION, "name": "select_worker"}]})
            elif selected == FOCUSED_TEST:
                result(request["id"], {"qualified_name": FOCUSED_TEST, "name": "keeps_affinity", "source": "focused test source"})
            else:
                raise AssertionError("unexpected source selector " + str(selected))
    else:
        send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown method"}})
"#;
