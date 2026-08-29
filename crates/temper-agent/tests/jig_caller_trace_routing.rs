use std::fs;
use std::path::Path;
use std::sync::Mutex;

use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::Value as JsonValue;
use temper_agent::{
    CodingAgentError, ProviderConfig, WorkspaceContext, WorkspaceGuidance, WorkspaceRepository,
    WorkspaceWorkItem, run_coding_agent_native_with_tool_config,
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
#[path = "jig_caller_trace_routing/context.rs"]
mod context;
use context::workspace_context;
#[path = "jig_caller_trace_routing/focused_test_fallback.rs"]
mod focused_test_fallback;
#[path = "jig_caller_trace_routing/incomplete_selector.rs"]
mod incomplete_selector;
#[path = "jig_caller_trace_routing/semantic_routing.rs"]
mod semantic_routing;
static JIG_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn jig_routes_empty_inbound_trace_and_exact_source_relationships() {
    let _serial = JIG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let checkout = TempCheckout::new("jig-caller-trace-implementation-root");
    checkout.init_git();
    seed_route_target(&checkout);
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
        fs::read_to_string(checkout.repo_path().join("ROUTE.md")).expect("routing product"),
        "caller route verified\n"
    );
    let calls = graph_calls(mcp.path());
    assert_eq!(
        tool_names(&calls),
        [
            "search_code",
            "get_code_snippet",
            "trace_path",
            "get_code_snippet",
            "search_graph",
            "get_code_snippet",
        ]
    );
    assert_eq!(calls[1]["arguments"]["qualified_name"], IMPLEMENTATION);
    assert_eq!(calls[2]["arguments"]["function_name"], IMPLEMENTATION);
    assert_eq!(calls[2]["arguments"]["direction"], "inbound");
    assert_eq!(calls[3]["arguments"]["qualified_name"], CALLER);
    assert_eq!(
        calls[4]["arguments"]["query"],
        "request affinity remains stable across worker selection"
    );
    assert_eq!(calls[5]["arguments"]["qualified_name"], FOCUSED_TEST);
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
    seed_route_target(&checkout);
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
        fs::read_to_string(checkout.repo_path().join("ROUTE.md")).expect("recovery product"),
        "denied trace was not repeated\n"
    );
    let calls = graph_calls(mcp.path());
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["name"] == "trace_path")
            .count(),
        1,
        "only the implementation caller trace may reach the provider"
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
    assert_eq!(
        calls
            .iter()
            .filter(|call| {
                call["name"] == "get_code_snippet" && call["arguments"]["qualified_name"] == CALLER
            })
            .count(),
        1,
        "speculative and repeated focused-test reads of the caller must stay local"
    );
}

fn seed_route_target(checkout: &TempCheckout) {
    fs::write(
        checkout.repo_path().join("ROUTE.md"),
        "pending exact read\n",
    )
    .expect("seed route target");
    checkout.git(&["add", "ROUTE.md"]);
    checkout.git(&["commit", "-m", "seed route target"]);
}

fn minimal_routing_reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0 => search_reply("discover-likely-implementation"),
        1 => source_reply(
            "read-selected-implementation",
            implementation_target(view),
            "implementation",
        ),
        2 => trace_reply("trace-selected-implementation", implementation_target(view)),
        3 => source_reply("read-returned-caller", caller_relationship(view), "caller"),
        4 => tool_reply(
            "search-focused-test-semantically",
            "codebase_memory_search_graph",
            serde_json::json!({
                "query": "request affinity remains stable across worker selection"
            }),
        ),
        5 => source_reply(
            "read-returned-focused-test",
            semantic_test_relationship(view),
            "focused_test",
        ),
        6 => tool_reply(
            "read-routed-target-after-evidence",
            "read",
            serde_json::json!({"path": "demo/ROUTE.md"}),
        ),
        7 => tool_reply(
            "write-after-routed-evidence",
            "write",
            serde_json::json!({
                "path": "demo/ROUTE.md",
                "content": "caller route verified\n"
            }),
        ),
        8 => Reply::text(
            r#"{"summary":"Selected the implementation root and consumed returned caller evidence."}"#,
        ),
        count => panic!("unexpected minimal routing tool-result count {count}"),
    }
}

fn recovery_routing_reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0 => search_reply("discover-recovery-implementation"),
        1 => source_reply(
            "read-recovery-implementation",
            implementation_target(view),
            "implementation",
        ),
        2 => trace_reply("trace-recovery-implementation", implementation_target(view)),
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
            source_reply(
                "recover-returned-caller",
                caller_relationship(view),
                "caller",
            )
        }
        6 => {
            assert!(messages_contain(view, "missing evidence: [focused_test]"));
            for expected in [
                "search_graph/graph_query/focused_test",
                "selector=task_semantic_query",
            ] {
                assert!(messages_contain(view, expected), "menu omitted {expected}");
            }
            tool_batch(&[
                (
                    "speculative-implementation-test-denied",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": caller_relationship(view),
                        "decision_evidence_kind": "focused_test"
                    }),
                ),
                (
                    "recover-test-selector-semantically",
                    "codebase_memory_search_graph",
                    serde_json::json!({
                        "query": "request affinity remains stable across worker selection"
                    }),
                ),
            ])
        }
        8 => tool_batch(&[
            (
                "repeat-speculative-implementation-test-denied",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": caller_relationship(view),
                    "decision_evidence_kind": "focused_test"
                }),
            ),
            (
                "recover-exact-returned-focused-test",
                "codebase_memory_get_code_snippet",
                serde_json::json!({
                    "qualified_name": semantic_test_relationship(view),
                    "decision_evidence_kind": "focused_test"
                }),
            ),
        ]),
        10 => tool_reply(
            "read-recovered-target-after-evidence",
            "read",
            serde_json::json!({"path": "demo/ROUTE.md"}),
        ),
        11 => tool_reply(
            "write-after-compatible-recovery",
            "write",
            serde_json::json!({
                "path": "demo/ROUTE.md",
                "content": "denied trace was not repeated\n"
            }),
        ),
        12 => Reply::text(
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

fn caller_relationship(view: &RequestView) -> String {
    provider_results(view)
        .into_iter()
        .find(|result| {
            result
                .pointer("/function/qualified_name")
                .and_then(JsonValue::as_str)
                == Some(IMPLEMENTATION)
        })
        .and_then(|result| {
            result
                .pointer("/callers/0/qualified_name")
                .and_then(JsonValue::as_str)
                .map(str::to_string)
        })
        .expect("exact implementation source returned a caller")
}

fn semantic_test_relationship(view: &RequestView) -> String {
    provider_results(view)
        .iter()
        .rev()
        .filter_map(|result| result.get("results").and_then(JsonValue::as_array))
        .flatten()
        .find_map(|result| {
            result
                .get("qualified_name")
                .and_then(JsonValue::as_str)
                .filter(|target| *target == FOCUSED_TEST)
                .map(str::to_string)
        })
        .expect("semantic search returned a focused test")
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

const FAKE_MCP: &str = r#"
import json
import os
import sys

IMPLEMENTATION = "crate::routing::select_worker"
CALLER = "crate::delivery::dispatch"
FOCUSED_TEST = "crate::tests::keeps_affinity"
SECOND_FOCUSED_TEST = "crate::tests::keeps_affinity_after_retry"
NON_TEST_HELPER = "crate::routing::request_affinity_helper"
SEMANTIC_IMPLEMENTATION = "crate::scheduler::choose_lane"
SEMANTIC_CALLER = "crate::replay::route_renamed_job"
SEMANTIC_FOCUSED_TEST = "crate::tests::renamed_replay_preserves_shard_ownership"
ACTIVE_ROOT_TEST = "crate::tests::replay_uses_selected_lane"
INCIDENTAL_IMPLEMENTATION = "crate::routing::legacy_dispatch_key"
CALL_LOG = os.path.join(os.path.dirname(__file__), "calls.jsonl")

TOOLS = [
    {"name": "search_graph", "description": "Targeted graph search", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "name_pattern": {"type": "string"}, "project": {"type": "string"}}}},
    {"name": "search_code", "description": "Targeted code search", "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}, "pattern": {"type": "string"}, "project": {"type": "string"}}, "required": ["query"]}},
    {"name": "trace_path", "description": "Targeted caller trace", "inputSchema": {"type": "object", "properties": {"function_name": {"type": "string"}, "mode": {"type": "string"}, "direction": {"type": "string"}, "include_tests": {"type": "boolean"}, "depth": {"type": "integer"}, "project": {"type": "string"}}, "required": ["function_name"]}},
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
        elif name == "search_graph":
            if arguments.get("query") == "renamed job replay shard ownership":
                result(request["id"], {"results": [{"qualified_name": SEMANTIC_IMPLEMENTATION, "name": "choose_lane"}, {"qualified_name": SEMANTIC_CALLER, "name": "route_renamed_job"}, {"qualified_name": ACTIVE_ROOT_TEST, "name": "replay_uses_selected_lane", "is_test": True}]})
            elif arguments.get("query") == "renamed replay shard ownership regression":
                result(request["id"], {"results": [{"qualified_name": SEMANTIC_FOCUSED_TEST, "name": "renamed_replay_preserves_shard_ownership", "is_test": True}]})
            elif arguments.get("query") == "request affinity remains stable across worker selection":
                result(request["id"], {"results": [{"qualified_name": FOCUSED_TEST, "name": "keeps_affinity", "label": "Function", "file_path": "tests/request_affinity.rs", "degree": 1}, {"qualified_name": SECOND_FOCUSED_TEST, "name": "keeps_affinity_after_retry", "label": "Function", "file_path": "tests/request_affinity.rs", "degree": 1}, {"qualified_name": NON_TEST_HELPER, "name": "request_affinity_helper", "label": "Function", "file_path": "src/route.rs", "degree": 2}], "total": 3, "has_more": False})
            elif arguments.get("query") == "request affinity regression without a matching test":
                result(request["id"], {"results": []})
            elif arguments.get("query") == "request affinity regression with ambiguous coverage":
                result(request["id"], {"results": [{"qualified_name": FOCUSED_TEST, "function_name": "different_test_identity", "label": "Function"}]})
            elif arguments.get("query") == "request affinity regression with malformed coverage":
                result(request["id"], {"results": [{"qualified_name": ["not", "an", "identity"]}]})
            elif arguments.get("query") == "request affinity regression returning a non-test helper":
                result(request["id"], {"results": [{"qualified_name": NON_TEST_HELPER, "name": "request_affinity_helper", "label": "Function"}]})
            elif arguments.get("name_pattern") == "dispatch_key":
                result(request["id"], {"results": [{"qualified_name": INCIDENTAL_IMPLEMENTATION, "name": "legacy_dispatch_key"}]})
            else:
                raise AssertionError("unexpected graph search " + str(arguments))
        elif name == "search_code":
            selected = arguments.get("pattern")
            if selected == SEMANTIC_IMPLEMENTATION:
                result(request["id"], {"results": [{"qualified_name": SEMANTIC_IMPLEMENTATION, "name": "choose_lane"}]})
            else:
                result(request["id"], {"results": [{"qualified_name": IMPLEMENTATION, "name": "select_worker"}]})
        elif name == "trace_path":
            selected = arguments.get("function_name")
            assert arguments.get("direction") == "inbound"
            if selected == IMPLEMENTATION:
                assert not arguments.get("include_tests", False)
                result(request["id"], {"function": {"qualified_name": IMPLEMENTATION, "name": "select_worker"}, "direction": "inbound", "complete": True, "callers": [{"qualified_name": CALLER, "name": "dispatch"}]})
            elif selected == CALLER:
                assert arguments.get("mode") == "calls"
                assert arguments.get("include_tests") is True
                if arguments.get("depth") == 1:
                    result(request["id"], {"function": {"qualified_name": CALLER, "name": "dispatch"}, "mode": "calls", "direction": "inbound", "include_tests": True, "complete": True, "callers": []})
                else:
                    result(request["id"], {"function": {"qualified_name": CALLER, "name": "dispatch"}, "mode": "calls", "direction": "inbound", "include_tests": True, "complete": True, "callers": [{"qualified_name": FOCUSED_TEST, "name": "keeps_affinity", "is_test": True}]})
            elif selected == SEMANTIC_IMPLEMENTATION:
                assert not arguments.get("include_tests", False)
                result(request["id"], {"function": {"qualified_name": SEMANTIC_IMPLEMENTATION, "name": "choose_lane"}, "direction": "inbound", "complete": True, "callers": [{"qualified_name": SEMANTIC_CALLER, "name": "route_renamed_job"}]})
            elif selected == SEMANTIC_CALLER:
                assert arguments.get("mode") == "calls"
                assert arguments.get("include_tests") is True
                result(request["id"], {"function": {"qualified_name": SEMANTIC_CALLER, "name": "route_renamed_job"}, "mode": "calls", "direction": "inbound", "include_tests": True, "complete": True, "callers": []})
            else:
                raise AssertionError("unexpected trace selector " + str(selected))
        elif name == "get_code_snippet":
            selected = arguments.get("qualified_name")
            if selected == IMPLEMENTATION:
                result(request["id"], {"qualified_name": IMPLEMENTATION, "name": "select_worker", "file_path": "ROUTE.md", "source": "implementation source", "callers": [{"qualified_name": CALLER, "name": "dispatch"}]})
            elif selected == CALLER:
                result(request["id"], {"qualified_name": CALLER, "name": "dispatch", "file_path": "ROUTE.md", "source": "caller source", "callees": [{"qualified_name": IMPLEMENTATION, "name": "select_worker"}]})
            elif selected == FOCUSED_TEST:
                result(request["id"], {"qualified_name": FOCUSED_TEST, "name": "keeps_affinity", "file_path": "ROUTE.md", "source": "focused test source", "is_test": True})
            elif selected == SECOND_FOCUSED_TEST:
                result(request["id"], {"qualified_name": SECOND_FOCUSED_TEST, "name": "keeps_affinity_after_retry", "file_path": "ROUTE.md", "source": "second focused test source", "is_test": True})
            elif selected == NON_TEST_HELPER:
                result(request["id"], {"qualified_name": NON_TEST_HELPER, "name": "request_affinity_helper", "file_path": "ROUTE.md", "source": "non-test helper source", "is_test": False})
            elif selected == SEMANTIC_IMPLEMENTATION:
                result(request["id"], {"qualified_name": SEMANTIC_IMPLEMENTATION, "name": "choose_lane", "file_path": "ROUTE.md", "source": "semantic implementation source"})
            elif selected == SEMANTIC_CALLER:
                result(request["id"], {"qualified_name": SEMANTIC_CALLER, "name": "route_renamed_job", "file_path": "ROUTE.md", "source": "semantic caller source", "callees": [{"qualified_name": SEMANTIC_IMPLEMENTATION, "name": "choose_lane"}]})
            elif selected == SEMANTIC_FOCUSED_TEST:
                result(request["id"], {"qualified_name": SEMANTIC_FOCUSED_TEST, "name": "renamed_replay_preserves_shard_ownership", "file_path": "ROUTE.md", "source": "semantic focused test source", "is_test": True})
            elif selected == ACTIVE_ROOT_TEST:
                result(request["id"], {"qualified_name": ACTIVE_ROOT_TEST, "name": "replay_uses_selected_lane", "file_path": "ROUTE.md", "source": "active-root focused test source", "is_test": True})
            elif selected == INCIDENTAL_IMPLEMENTATION:
                result(request["id"], {"qualified_name": INCIDENTAL_IMPLEMENTATION, "name": "legacy_dispatch_key", "file_path": "ROUTE.md", "source": "incidental implementation source"})
            else:
                raise AssertionError("unexpected source selector " + str(selected))
    else:
        send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown method"}})
"#;
