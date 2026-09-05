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

#[path = "jig_caller_trace_routing/context.rs"]
mod context;
use context::workspace_context;

const IMPLEMENTATION: &str = "crate::scheduler::choose_lane";
const CALLER: &str = "crate::replay::route_renamed_job";
const FOCUSED_TEST: &str = "crate::tests::renamed_replay_preserves_shard_ownership";
static JIG_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn jig_completes_independent_implementation_and_focused_test_roots() {
    let _serial = JIG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let checkout = TempCheckout::new("jig-independent-decision-roots");
    checkout.init_git();
    seed_route_target(&checkout);
    let mcp = fake_mcp();
    let fake = FakeLlm::start(Script::rule(independent_roots_reply)).expect("start fake LLM");
    let provider = provider(&fake, "jig-independent-decision-roots");
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
    .expect("independent provider-derived roots complete");

    assert_eq!(result.verdict, None);
    assert_eq!(
        fs::read_to_string(checkout.repo_path().join("ROUTE.md")).unwrap(),
        "independent roots verified\n"
    );
    let calls = graph_calls(mcp.path());
    assert_eq!(
        tool_names(&calls),
        [
            "search_graph",
            "search_graph",
            "get_code_snippet",
            "trace_path",
            "get_code_snippet",
            "get_code_snippet",
        ]
    );
    assert_eq!(calls[2]["arguments"]["qualified_name"], IMPLEMENTATION);
    assert_eq!(calls[3]["arguments"]["function_name"], IMPLEMENTATION);
    assert_eq!(calls[4]["arguments"]["qualified_name"], CALLER);
    assert_eq!(calls[5]["arguments"]["qualified_name"], FOCUSED_TEST);
}

#[test]
fn jig_success_without_a_provider_focused_test_root_stops_without_product() {
    let _serial = JIG_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let checkout = TempCheckout::new("jig-missing-focused-root");
    checkout.init_git();
    seed_route_target(&checkout);
    let mcp = fake_mcp();
    let fake = FakeLlm::start(Script::rule(incomplete_roots_reply)).expect("start fake LLM");
    let provider = provider(&fake, "jig-missing-focused-root");
    let config = tool_config(&mcp);
    let context = workspace_context();
    let cwd = checkout.path().to_path_buf();

    let error = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_coding_agent_native_with_tool_config(
            handle,
            &provider,
            &context,
            &cwd,
            7,
            None,
            Some(&config),
        )
        .await
    })
    .expect_err("enabled evidence without a focused root cannot fall back");
    assert!(matches!(
        error,
        CodingAgentError::DecisionAnchorRecoveryExhausted | CodingAgentError::NoProduct
    ));
    assert_eq!(
        fs::read_to_string(checkout.repo_path().join("ROUTE.md")).unwrap(),
        "pending exact read\n"
    );
    assert!(
        graph_calls(mcp.path())
            .iter()
            .all(|call| call["name"] != "search_graph"),
        "incomplete enabled recovery cannot invent a semantic query"
    );
}

fn independent_roots_reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0 => tool_batch(&[
            (
                "implementation-root",
                "codebase_memory_search_graph",
                serde_json::json!({"query": "renamed job replay shard ownership"}),
            ),
            (
                "focused-root",
                "codebase_memory_search_graph",
                serde_json::json!({"query": "renamed replay shard ownership regression"}),
            ),
        ]),
        2 => source_reply(
            "implementation",
            &active_handoff_candidate(view, 1),
            "implementation",
        ),
        3 => tool_reply(
            "trace",
            "codebase_memory_trace_path",
            serde_json::json!({"function_name": IMPLEMENTATION, "direction": "inbound"}),
        ),
        4 => source_reply("caller", CALLER, "caller"),
        5 => {
            assert!(messages_contain(
                view,
                "focused_test/selector=focused_test_result"
            ));
            source_reply("focused-test", FOCUSED_TEST, "focused_test")
        }
        6 => tool_reply(
            "exact-read",
            "read",
            serde_json::json!({"path": "demo/ROUTE.md"}),
        ),
        7 => tool_reply(
            "write",
            "write",
            serde_json::json!({
                "path": "demo/ROUTE.md",
                "content": "independent roots verified\n"
            }),
        ),
        8 => Reply::text(r#"{"summary":"Retained both provider-derived root lineages."}"#),
        count => panic!("unexpected independent-root tool-result count {count}"),
    }
}

fn incomplete_roots_reply(view: &RequestView) -> Reply {
    match view.prior_tool_results {
        0 => tool_reply(
            "implementation-root",
            "codebase_memory_search_code",
            serde_json::json!({"query": "worker selection", "pattern": "select worker"}),
        ),
        1 => source_reply(
            "implementation",
            "crate::routing::select_worker",
            "implementation",
        ),
        2 => tool_reply(
            "trace",
            "codebase_memory_trace_path",
            serde_json::json!({
                "function_name": "crate::routing::select_worker",
                "direction": "inbound"
            }),
        ),
        3 => source_reply("caller", "crate::delivery::dispatch", "caller"),
        count => panic!(
            "incomplete enabled evidence should stop before another model turn; count={count}; messages={:?}",
            view.messages
                .iter()
                .map(|message| &message.content)
                .collect::<Vec<_>>()
        ),
    }
}

fn source_reply(id: &str, qualified_name: &str, kind: &str) -> Reply {
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

fn messages_contain(view: &RequestView, needle: &str) -> bool {
    view.messages
        .iter()
        .any(|message| message.content.contains(needle))
}

fn active_handoff_candidate(view: &RequestView, index: usize) -> String {
    let label = format!("candidate_{index}=");
    view.messages
        .iter()
        .rev()
        .find_map(|message| {
            message
                .content
                .split_once(&label)
                .and_then(|(_, value)| value.split([',', ']', ' ']).next())
                .filter(|reference| reference.starts_with("temper-recovery-selector:"))
                .map(str::to_string)
        })
        .expect("active-root candidate handoff")
}

fn seed_route_target(checkout: &TempCheckout) {
    fs::write(
        checkout.repo_path().join("ROUTE.md"),
        "pending exact read\n",
    )
    .unwrap();
    checkout.git(&["add", "ROUTE.md"]);
    checkout.git(&["commit", "-m", "seed route target"]);
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

const FAKE_MCP: &str = include_str!("jig_caller_trace_routing/fake_mcp.py");
