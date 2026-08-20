use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use jig_core::{Reply, Script, StopReason, Turn};
use jig_server::FakeLlm;
use temper_agent::{
    ForgeContextHost, ProviderConfig, SubmitForPrHost, WorkspaceContext, WorkspaceGuidance,
    WorkspaceRepository, WorkspaceWorkItem,
    run_coding_agent_native_with_totals_tool_config_and_hosts,
};
use temper_protocol_agent::{
    AgentRuntimeLimitsV1, AgentToolConfig, CodebaseMemoryIndex, CodebaseMemoryMode,
    CodebaseMemoryToolConfig, ForgeContextResult, SubmitForPrResponse,
};

#[path = "support/coding_agent_workspace.rs"]
mod coding_agent_workspace;
use coding_agent_workspace::{REPO_DIR, TempCheckout};

#[test]
fn jig_native_loop_redirects_an_identical_ordinary_failure_and_recovers() {
    let checkout = TempCheckout::new("jig-ordinary-recovery-after-graph-closure");
    checkout.init_git();
    let attempt_path = checkout.repo_path().join("ordinary-attempts.log");
    let fake = ordinary_recovery_fake(attempt_path.clone());
    let provider = ProviderConfig::anthropic_oauth(Some(jig_auth_fixture()))
        .with_base_url_override(fake.base_url());
    let mcp_dir = fake_codebase_memory_mcp_script();
    let tool_config = codebase_memory_tool_config(&mcp_dir);
    let submit_calls = Arc::new(AtomicUsize::new(0));
    let forge_calls = Arc::new(AtomicUsize::new(0));
    let submit = accepting_submit_host(
        Arc::clone(&submit_calls),
        "RECOVERY.md",
        "ordinary recovery converged\n",
    );
    let forge = synthetic_forge_host(Arc::clone(&forge_calls));
    let context = workspace_context();
    let cwd = checkout.path().to_path_buf();

    let result = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_coding_agent_native_with_totals_tool_config_and_hosts(
            handle,
            &provider,
            &context,
            &cwd,
            12,
            None,
            false,
            Some(&tool_config),
            Some(submit),
            Some(forge),
            Default::default(),
            AgentRuntimeLimitsV1::default(),
        )
        .await
    })
    .expect("native loop converges after redirect")
    .0;

    let provider_transcript = fake
        .requests()
        .into_iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "FAKE_MCP_ARCHITECTURE_RESULT",
        "codebase-memory exploration is closed for this run",
        "# demo",
        "repeats a non-retryable failure",
        "Synthetic fixture context",
        "submit_for_pr accepted by host",
    ] {
        assert!(
            provider_transcript.contains(expected),
            "Anthropic provider continuation omitted {expected:?}"
        );
    }

    assert_eq!(result.verdict, None);
    assert_eq!(
        fs::read_to_string(checkout.repo_path().join("RECOVERY.md")).expect("recovery product"),
        "ordinary recovery converged\n"
    );
    assert_eq!(
        fs::read_to_string(&attempt_path).expect("execution counter"),
        "attempt\n",
        "the identical retry must settle locally without executing bash twice"
    );
    assert_eq!(submit_calls.load(Ordering::SeqCst), 1);
    assert_eq!(forge_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn jig_graph_closure_and_graph_circuit_leave_ordinary_tools_available() {
    let checkout = TempCheckout::new("jig-graph-closure-locality");
    checkout.init_git();
    let fake = graph_locality_fake();
    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-graph-closure-locality",
        "https://example.invalid/unused-production-url",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url());
    let mcp_dir = fake_codebase_memory_mcp_script();
    let tool_config = codebase_memory_tool_config(&mcp_dir);
    let submit_calls = Arc::new(AtomicUsize::new(0));
    let forge_calls = Arc::new(AtomicUsize::new(0));
    let submit = accepting_submit_host(
        Arc::clone(&submit_calls),
        "CLOSURE.md",
        "graph closure stayed local\n",
    );
    let forge = synthetic_forge_host(Arc::clone(&forge_calls));
    let context = workspace_context();
    let cwd = checkout.path().to_path_buf();

    let result = temper_agent_io::block_on_with(move |_cx, handle| async move {
        run_coding_agent_native_with_totals_tool_config_and_hosts(
            handle,
            &provider,
            &context,
            &cwd,
            10,
            None,
            false,
            Some(&tool_config),
            Some(submit),
            Some(forge),
            Default::default(),
            AgentRuntimeLimitsV1::default(),
        )
        .await
    })
    .expect("ordinary tools complete after graph-local stops")
    .0;

    assert_eq!(result.verdict, None);
    assert_eq!(
        fs::read_to_string(checkout.repo_path().join("CLOSURE.md")).expect("closure product"),
        "graph closure stayed local\n"
    );
    assert_eq!(submit_calls.load(Ordering::SeqCst), 1);
    assert_eq!(forge_calls.load(Ordering::SeqCst), 1);
}

fn ordinary_recovery_fake(attempt_path: PathBuf) -> FakeLlm {
    const FAILED_COMMAND: &str = "printf 'attempt\\n' >> demo/ordinary-attempts.log; exit 9";
    let script = Script::rule(move |view| match view.prior_tool_results {
        0 => tool_reply(
            "graph-discovery",
            "codebase_memory_get_architecture",
            serde_json::json!({}),
        ),
        1 => tool_reply(
            "close-graph-exploration",
            "codebase_memory_get_architecture",
            serde_json::json!({"close": true}),
        ),
        2 => tool_reply(
            "provider-native-read",
            "Read",
            serde_json::json!({"file_path": "demo/README.md"}),
        ),
        3 => tool_reply(
            "malformed-shell-operation",
            "Bash",
            serde_json::json!({"command": FAILED_COMMAND}),
        ),
        4 => {
            assert_eq!(
                fs::read_to_string(&attempt_path).expect("first bash counter"),
                "attempt\n"
            );
            tool_reply(
                "identical-malformed-shell-operation",
                "Bash",
                serde_json::json!({"command": FAILED_COMMAND}),
            )
        }
        5 => {
            assert_eq!(
                fs::read_to_string(&attempt_path).expect("redirect counter"),
                "attempt\n",
                "redirected retry reached the underlying bash tool"
            );
            tool_replies(&[
                (
                    "corrected-provider-native-write",
                    "Write",
                    serde_json::json!({
                        "file_path": "demo/RECOVERY.md",
                        "content": "ordinary recovery converged\n"
                    }),
                ),
                (
                    "validate-correction",
                    "Bash",
                    serde_json::json!({
                        "command": "test \"$(cat demo/RECOVERY.md)\" = 'ordinary recovery converged'"
                    }),
                ),
                (
                    "read-forge-after-closure",
                    "forge_get_item",
                    serde_json::json!({"repo":"acme/demo","number":25,"type":"issue"}),
                ),
                (
                    "submit-correction",
                    "submit_for_pr",
                    serde_json::json!({"summary":"ordinary recovery ready"}),
                ),
            ])
        }
        9 => Reply::text(
            r#"{"title":"Recover ordinary tools after graph closure","body":"Validated recovery.","summary":"Recovered and submitted."}"#,
        ),
        count => panic!("unexpected ordinary-recovery tool-result count {count}"),
    });
    FakeLlm::start(script).expect("start ordinary-recovery fake LLM")
}

fn graph_locality_fake() -> FakeLlm {
    FakeLlm::start(Script::rule(move |view| {
        let saw = |needle: &str| {
            view.messages
                .iter()
                .any(|message| message.role == "tool" && message.content.contains(needle))
        };
        match view.prior_tool_results {
            0 => tool_reply(
                "closed-graph",
                "codebase_memory_get_architecture",
                serde_json::json!({"close": true}),
            ),
            1 => {
                assert!(saw("codebase-memory exploration is closed for this run"));
                tool_reply(
                    "systemic-graph-failure",
                    "codebase_memory_get_architecture",
                    serde_json::json!({"systemic": true}),
                )
            }
            2 => {
                assert!(saw("provider or protocol request failed"));
                tool_reply(
                    "graph-circuit-open",
                    "codebase_memory_get_architecture",
                    serde_json::json!({}),
                )
            }
            3 => {
                assert!(saw("codebase-memory is disabled for this run"));
                tool_replies(&[
                    (
                        "write-after-graph-stops",
                        "write",
                        serde_json::json!({
                            "path": "demo/CLOSURE.md",
                            "content": "graph closure stayed local\n"
                        }),
                    ),
                    (
                        "validate-after-graph-stops",
                        "bash",
                        serde_json::json!({
                            "command": "test \"$(cat demo/CLOSURE.md)\" = 'graph closure stayed local'"
                        }),
                    ),
                    (
                        "forge-after-graph-stops",
                        "forge_get_item",
                        serde_json::json!({"repo":"acme/demo","number":25}),
                    ),
                    (
                        "submit-after-graph-stops",
                        "submit_for_pr",
                        serde_json::json!({"summary":"graph-local recovery ready"}),
                    ),
                ])
            }
            7 => {
                assert!(saw("Synthetic fixture context"));
                assert!(saw("submit_for_pr accepted by host"));
                Reply::text(
                    r#"{"title":"Keep graph closure local","body":"Validated graph-local recovery.","summary":"Ordinary tools remained available."}"#,
                )
            }
            count => panic!("unexpected graph-locality tool-result count {count}"),
        }
    }))
    .expect("start graph-locality fake LLM")
}

fn tool_reply(id: &str, name: &str, args: serde_json::Value) -> Reply {
    tool_replies(&[(id, name, args)])
}

fn tool_replies(calls: &[(&str, &str, serde_json::Value)]) -> Reply {
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

fn accepting_submit_host(
    calls: Arc<AtomicUsize>,
    file_name: &'static str,
    expected: &'static str,
) -> SubmitForPrHost {
    Arc::new(move |_request, _context, cwd| {
        calls.fetch_add(1, Ordering::SeqCst);
        let accepted = fs::read_to_string(cwd.join(REPO_DIR).join(file_name))
            .is_ok_and(|content| content == expected);
        Box::pin(std::future::ready(SubmitForPrResponse {
            accepted,
            message: if accepted {
                "synthetic gate accepted corrected workspace"
            } else {
                "synthetic gate rejected unvalidated workspace"
            }
            .to_string(),
            gates: Vec::new(),
        }))
    })
}

fn synthetic_forge_host(calls: Arc<AtomicUsize>) -> ForgeContextHost {
    Arc::new(move |_operation| {
        calls.fetch_add(1, Ordering::SeqCst);
        let result: ForgeContextResult = serde_json::from_value(serde_json::json!({
            "result":"item",
            "item":{
                "artifact":{
                    "repository":{"id":"fixture-repo","path":"acme/demo"},
                    "artifact_type":"issue",
                    "number":25
                },
                "title":"Synthetic fixture context",
                "body":"Synthetic, privacy-safe context.",
                "state":"open"
            },
            "truncation":{
                "depth_exceeded":false,
                "count_exceeded":false,
                "content_truncated":false
            }
        }))
        .expect("synthetic Forge result parses");
        Box::pin(std::future::ready(Ok(result)))
    })
}

fn fake_codebase_memory_mcp_script() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(
        dir.path().join("fake_graph_closure_mcp.py"),
        r#"
import json
import sys

TOOLS = [
    {"name": "get_architecture", "description": "Synthetic architecture discovery", "inputSchema": {"type": "object", "properties": {"close": {"type": "boolean"}, "systemic": {"type": "boolean"}, "project": {"type": "string"}}}},
    {"name": "index_status", "description": "Index status", "inputSchema": {"type": "object", "properties": {"project": {"type": "string"}}, "required": ["project"]}},
    {"name": "index_repository", "description": "Stable repository upsert", "inputSchema": {"type": "object", "properties": {"repo_path": {"type": "string"}, "name": {"type": "string"}}, "required": ["repo_path"]}},
]

def send(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

def tool_result(request_id, text, is_error=False):
    send({"jsonrpc": "2.0", "id": request_id, "result": {"content": [{"type": "text", "text": text}], "isError": is_error}})

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
        params = request.get("params", {})
        name = params.get("name")
        args = params.get("arguments") or {}
        if name == "index_status":
            tool_result(request["id"], json.dumps({"project": args.get("project", ""), "status": "fresh"}))
        elif name == "get_architecture" and args.get("close") is True:
            tool_result(request["id"], "exploration_closed", True)
        elif name == "get_architecture" and args.get("systemic") is True:
            tool_result(request["id"], "synthetic provider failure", True)
        elif name == "get_architecture":
            tool_result(request["id"], "FAKE_MCP_ARCHITECTURE_RESULT")
        else:
            tool_result(request["id"], json.dumps({"status": "indexed"}))
    else:
        send({"jsonrpc": "2.0", "id": request["id"], "error": {"code": -32601, "message": "unknown method"}})
"#,
    )
    .expect("write fake graph-closure MCP server");
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
                    .join("fake_graph_closure_mcp.py")
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
                    "title": "Exercise graph-local recovery",
                    "body": "Create the requested recovery file and validate it.",
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
            role_guidance: Some("Create and validate the requested recovery product.".to_string()),
            tool_guidance: Some("Use the available workspace tools.".to_string()),
            tool_constraints: vec!["Do not run git commit.".to_string()],
            action_guidance: None,
        },
        pull_request_freshness: None,
        agent_session: None,
    }
}

fn jig_auth_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jig_auth.json")
}
