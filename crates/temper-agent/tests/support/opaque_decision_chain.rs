//! Native-Jig coverage for result-derived graph decision chains.
//!
//! The MCP fixture mints opaque values at runtime. The fake can use a dependent
//! value only after it appears in a prior tool message, so a fixed successful
//! sequence cannot accidentally stand in for evidence consumption.

use std::fs;
use std::sync::{Arc, Mutex, OnceLock};

use jig_core::{Reply, Script, StopReason, Turn};
use jig_server::FakeLlm;
use temper_agent::{
    CodingAgentError, ProviderConfig, WorkspaceContext, WorkspaceGuidance, WorkspaceRepository,
    WorkspaceWorkItem, run_coding_agent_native_with_tool_config,
};
use temper_protocol_agent::{
    AgentToolConfig, CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig,
};

#[path = "coding_agent_workspace.rs"]
mod coding_agent_workspace;
use coding_agent_workspace::{REPO_DIR, TempCheckout};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecisionCase {
    Consumed,
    UnrelatedLaterTarget,
    ProducerTurnDependents,
    ConventionalReadSubstitution,
    IncompleteSourceEvidence,
    UnavailableAfterRoot,
    UnconsumableRecoveryExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecisionStep {
    Discovery,
    Refinement,
    Trace,
    ImplementationSource,
    CallerSource,
    BehavioralTestSource,
    Mutation,
    MutationAttempt,
    MutationBlocked,
    UnrelatedLaterTarget,
    ProducerTurnDependents,
    UnavailableFallback,
    Recovery,
    BypassStopped,
    Complete,
}

#[derive(Debug, Eq, PartialEq)]
pub struct DecisionRun {
    pub steps: Vec<DecisionStep>,
    pub mutation: Option<String>,
}

static DECISION_CHAIN_RUN_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn provider_result(text: &str) -> Option<serde_json::Value> {
    let result = text
        .split_once("\n\n[Decision anchor:")
        .map_or(text, |(result, _)| result);
    serde_json::from_str(result).ok()
}

pub fn run(case: DecisionCase) -> DecisionRun {
    let _serial = DECISION_CHAIN_RUN_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("decision-chain run lock");
    let checkout = TempCheckout::new("jig-opaque-decision-chain");
    checkout.init_git();

    let observed_steps = Arc::new(Mutex::new(Vec::new()));
    let fake = decision_chain_fake(case, Arc::clone(&observed_steps));
    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "jig-opaque-decision-chain",
        "https://example.invalid/unused-production-url",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url());
    let mcp_dir = opaque_decision_mcp();
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
    });
    match (case, result) {
        (DecisionCase::Consumed | DecisionCase::UnavailableAfterRoot, Ok(result)) => {
            assert_eq!(result.verdict, None)
        }
        (DecisionCase::Consumed | DecisionCase::UnavailableAfterRoot, Err(error)) => {
            panic!("native Jig agent completes the consumed decision chain: {error}")
        }
        (
            _,
            Err(CodingAgentError::NoProduct | CodingAgentError::DecisionAnchorRecoveryExhausted),
        ) => {}
        (_, Ok(result)) => {
            panic!("a stopped bypass must not produce a landable result: {result:?}")
        }
        (_, Err(error)) => panic!("native Jig agent rejects the bypass without mutation: {error}"),
    }

    DecisionRun {
        steps: observed_steps.lock().expect("decision steps lock").clone(),
        mutation: fs::read_to_string(checkout.repo_path().join("EVIDENCE.md")).ok(),
    }
}

fn decision_chain_fake(
    case: DecisionCase,
    observed_steps: Arc<Mutex<Vec<DecisionStep>>>,
) -> FakeLlm {
    FakeLlm::start(Script::rule(move |view| {
        let provider_values = |field: &str| {
            view.messages
                .iter()
                .filter_map(|message| {
                    if message.role != "tool" {
                        return None;
                    }
                    let pointer = format!("/results/0/{field}");
                    provider_result(&message.content)?
                        .pointer(&pointer)
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .collect::<Vec<_>>()
        };
        let result_count = || {
            view.messages
                .iter()
                .filter(|message| {
                    message.role == "tool"
                        && provider_result(&message.content)
                            .is_some_and(|value| value.get("results").is_some())
                })
                .count()
        };
        let next_target = || {
            provider_values("next")
                .pop()
                .expect("the prior provider-shaped result selected a next target")
        };
        let record = |step| observed_steps.lock().expect("decision steps lock").push(step);
        let mutation_was_blocked = || {
            view.messages.iter().any(|message| {
                message.role == "tool"
                    && message
                        .content
                        .contains("workspace mutation blocked until the successful decision anchor")
            })
        };

        match (case, view.prior_tool_results) {
            (DecisionCase::Consumed, 0) => {
                record(DecisionStep::Discovery);
                tool_reply(
                    "discover-implementation",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "implementation"}),
                )
            }
            (DecisionCase::Consumed, 1) => {
                assert!(
                    !provider_values("current_root").is_empty(),
                    "refinement requires a consumed current-root implementation result"
                );
                record(DecisionStep::Refinement);
                tool_reply(
                    "refine-implementation",
                    "codebase_memory_search_code",
                    serde_json::json!({"pattern": next_target()}),
                )
            }
            (DecisionCase::Consumed, 2) => {
                record(DecisionStep::Trace);
                tool_reply(
                    "trace-caller-or-model",
                    "codebase_memory_trace_path",
                    serde_json::json!({"function_name": next_target()}),
                )
            }
            (DecisionCase::Consumed, 3) => {
                record(DecisionStep::ImplementationSource);
                tool_reply(
                    "read-implementation",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": next_target(),
                        "decision_evidence_kind": "implementation",
                    }),
                )
            }
            (DecisionCase::Consumed, 4) => {
                record(DecisionStep::CallerSource);
                tool_reply(
                    "read-caller",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": next_target(),
                        "decision_evidence_kind": "caller",
                    }),
                )
            }
            (DecisionCase::Consumed, 5) => {
                record(DecisionStep::BehavioralTestSource);
                tool_reply(
                    "read-behavioral-test",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": next_target(),
                        "decision_evidence_kind": "focused_test",
                    }),
                )
            }
            (DecisionCase::Consumed, 6) => {
                assert!(
                    !provider_values("current_root").is_empty()
                        && provider_values("caller_model").len() == 1
                        && provider_values("implementation_source").len() == 1
                        && provider_values("behavioral_test").len() == 2,
                    "mutation requires consumed current-root, caller/model, and focused behavioral-test evidence"
                );
                record(DecisionStep::Mutation);
                tool_reply(
                    "mutate-after-evidence",
                    "write",
                    serde_json::json!({
                        "path": "demo/EVIDENCE.md",
                        "content": "verified evidence\n"
                    }),
                )
            }
            (DecisionCase::Consumed, 7) => {
                record(DecisionStep::Complete);
                Reply::text(r#"{"summary":"Mutated after consumed result-derived evidence."}"#)
            }
            (DecisionCase::UnrelatedLaterTarget, 0) => {
                record(DecisionStep::Discovery);
                tool_reply(
                    "discover-implementation",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "implementation"}),
                )
            }
            (DecisionCase::UnrelatedLaterTarget, 1) => {
                assert!(
                    !provider_values("current_root").is_empty(),
                    "the unrelated-target case starts after a successful producer"
                );
                record(DecisionStep::UnrelatedLaterTarget);
                tool_reply(
                    "refine-unrelated-target",
                    "codebase_memory_search_code",
                    serde_json::json!({"pattern": "not-derived"}),
                )
            }
            (DecisionCase::UnrelatedLaterTarget, 2) => {
                assert_eq!(
                    result_count(), 2,
                    "the unrelated target still receives a provider-shaped successful result"
                );
                record(DecisionStep::MutationAttempt);
                tool_reply(
                    "blocked-unrelated-mutation",
                    "write",
                    serde_json::json!({
                        "path": "demo/EVIDENCE.md",
                        "content": "must not be written\n"
                    }),
                )
            }
            (DecisionCase::UnrelatedLaterTarget, 3) => {
                assert!(mutation_was_blocked(), "the core must deny the unrelated mutation");
                record(DecisionStep::MutationBlocked);
                record(DecisionStep::Complete);
                Reply::text(r#"{"summary":"Stopped after a blocked unconsumed decision-chain mutation."}"#)
            }
            (DecisionCase::ProducerTurnDependents, 0) => {
                record(DecisionStep::ProducerTurnDependents);
                tool_replies(&[
                    (
                        "discover-implementation",
                        "codebase_memory_search_graph",
                        serde_json::json!({"query": "implementation"}),
                    ),
                    (
                        "refine-without-result",
                        "codebase_memory_search_code",
                        serde_json::json!({"pattern": "not-derived"}),
                    ),
                    (
                        "trace-without-result",
                        "codebase_memory_trace_path",
                        serde_json::json!({"function_name": "not-derived"}),
                    ),
                    (
                        "read-without-result",
                        "codebase_memory_get_code_snippet",
                        serde_json::json!({"qualified_name": "not-derived"}),
                    ),
                ])
            }
            (DecisionCase::ProducerTurnDependents, 4) => {
                assert_eq!(
                    result_count(), 4,
                    "producer-turn refinement, trace, and source reads all returned successfully"
                );
                record(DecisionStep::MutationAttempt);
                tool_reply(
                    "blocked-producer-mutation",
                    "write",
                    serde_json::json!({
                        "path": "demo/EVIDENCE.md",
                        "content": "must not be written\n"
                    }),
                )
            }
            (DecisionCase::ProducerTurnDependents, 5) => {
                assert!(mutation_was_blocked(), "the core must deny the producer-turn mutation");
                record(DecisionStep::MutationBlocked);
                record(DecisionStep::Complete);
                Reply::text(r#"{"summary":"Stopped after a blocked producer-turn mutation."}"#)
            }
            (DecisionCase::ConventionalReadSubstitution, 0) => {
                record(DecisionStep::Discovery);
                tool_reply(
                    "discover-implementation",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "implementation"}),
                )
            }
            (DecisionCase::ConventionalReadSubstitution, 1) => {
                record(DecisionStep::ImplementationSource);
                tool_reply("conventional-read", "read", serde_json::json!({"path": "demo/README.md"}))
            }
            (DecisionCase::ConventionalReadSubstitution, 2) => {
                record(DecisionStep::MutationAttempt);
                tool_reply(
                    "blocked-conventional-mutation",
                    "write",
                    serde_json::json!({
                        "path": "demo/EVIDENCE.md",
                        "content": "must not be written\n"
                    }),
                )
            }
            (DecisionCase::ConventionalReadSubstitution, 3) => {
                assert!(mutation_was_blocked(), "conventional reads must not consume the anchor");
                record(DecisionStep::MutationBlocked);
                record(DecisionStep::Complete);
                Reply::text(r#"{"summary":"Stopped after a blocked conventional-read substitution."}"#)
            }
            (DecisionCase::IncompleteSourceEvidence, 0) => {
                record(DecisionStep::Discovery);
                tool_reply(
                    "discover-implementation",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "implementation"}),
                )
            }
            (DecisionCase::IncompleteSourceEvidence, 1) => {
                record(DecisionStep::Refinement);
                tool_reply(
                    "refine-implementation",
                    "codebase_memory_search_code",
                    serde_json::json!({"pattern": next_target()}),
                )
            }
            (DecisionCase::IncompleteSourceEvidence, 2) => {
                record(DecisionStep::Trace);
                tool_reply(
                    "trace-caller-or-model",
                    "codebase_memory_trace_path",
                    serde_json::json!({"function_name": next_target()}),
                )
            }
            (DecisionCase::IncompleteSourceEvidence, 3) => {
                record(DecisionStep::ImplementationSource);
                tool_reply(
                    "read-implementation",
                    "codebase_memory_get_code_snippet",
                    serde_json::json!({
                        "qualified_name": next_target(),
                        "decision_evidence_kind": "implementation",
                    }),
                )
            }
            (DecisionCase::IncompleteSourceEvidence, 4) => {
                record(DecisionStep::MutationAttempt);
                tool_reply(
                    "blocked-incomplete-mutation",
                    "write",
                    serde_json::json!({
                        "path": "demo/EVIDENCE.md",
                        "content": "must not be written\n"
                    }),
                )
            }
            (DecisionCase::IncompleteSourceEvidence, 5) => {
                assert!(mutation_was_blocked(), "one source read must not satisfy the evidence gate");
                record(DecisionStep::MutationBlocked);
                record(DecisionStep::Complete);
                Reply::text(r#"{"summary":"Stopped after a blocked incomplete-evidence mutation."}"#)
            }
            (DecisionCase::UnavailableAfterRoot, 0) => {
                record(DecisionStep::Discovery);
                tool_reply(
                    "discover-implementation",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "implementation"}),
                )
            }
            (DecisionCase::UnavailableAfterRoot, 1) => {
                record(DecisionStep::Refinement);
                tool_reply(
                    "unavailable-refinement",
                    "codebase_memory_search_code",
                    serde_json::json!({
                        "pattern": next_target(),
                        "force_unavailable": true,
                    }),
                )
            }
            (DecisionCase::UnavailableAfterRoot, 2) => {
                assert!(
                    view.messages.iter().any(|message| {
                        message.role == "tool"
                            && message
                                .content
                                .contains("do not retry codebase-memory immediately")
                    }),
                    "the trusted unavailable result must provide bounded fallback guidance"
                );
                record(DecisionStep::UnavailableFallback);
                tool_reply(
                    "conventional-fallback-read",
                    "read",
                    serde_json::json!({"path": "demo/README.md"}),
                )
            }
            (DecisionCase::UnavailableAfterRoot, 3) => {
                record(DecisionStep::Mutation);
                tool_reply(
                    "mutate-after-unavailable-fallback",
                    "write",
                    serde_json::json!({
                        "path": "demo/EVIDENCE.md",
                        "content": "conventional fallback after unavailable provider\n",
                    }),
                )
            }
            (DecisionCase::UnavailableAfterRoot, 4) => {
                record(DecisionStep::Complete);
                Reply::text(r#"{"summary":"Used conventional discovery after an unavailable provider."}"#)
            }
            (DecisionCase::UnconsumableRecoveryExhausted, 0) => {
                record(DecisionStep::Discovery);
                tool_reply(
                    "discover-unconsumable",
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "unconsumable"}),
                )
            }
            (DecisionCase::UnconsumableRecoveryExhausted, turn @ 1..=2) => {
                let corrections = view
                    .messages
                    .iter()
                    .filter(|message| {
                        message.role == "user"
                            && message.content.contains("decision-anchor recovery required")
                    })
                    .collect::<Vec<_>>();
                assert!(
                    !corrections.is_empty(),
                    "the native agent must inject a generic recovery state"
                );
                assert!(
                    corrections
                        .iter()
                        .all(|message| message.content.contains("compatible current-root descendant")),
                    "recovery guidance must retain the bounded current-root policy"
                );
                assert!(
                    corrections
                        .iter()
                        .all(|message| !message.content.contains("PRIVATE-UNCONSUMABLE-SENTINEL")),
                    "recovery guidance must not retain provider values"
                );
                record(DecisionStep::Recovery);
                tool_reply(
                    &format!("recover-unconsumable-{turn}"),
                    "codebase_memory_search_graph",
                    serde_json::json!({"query": "unconsumable"}),
                )
            }
            (_, turn) => panic!("unexpected model turn {turn} for {case:?}"),
        }
    }))
    .expect("start opaque decision-chain fake LLM")
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

fn opaque_decision_mcp() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("MCP tempdir");
    fs::write(
        dir.path().join("opaque_decision_mcp.py"),
        include_str!("opaque_decision_mcp.py"),
    )
    .expect("write opaque decision MCP server");
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
                    .join("opaque_decision_mcp.py")
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
            branch_hint: Some("agent/pr-for-code-985".to_string()),
        }],
        work_item: WorkspaceWorkItem {
            role: "engineer".to_string(),
            queue: "code_ready".to_string(),
            kind: "code".to_string(),
            target: "Issue { number: ItemNumber(985) }".to_string(),
            context: "{}".to_string(),
        },
        action: "open_pr".to_string(),
        correlation_key: "pr-for-code-985".to_string(),
        checkout: Some("writable".to_string()),
        allowed_verdicts: vec!["needs_architect".to_string()],
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        guidance: WorkspaceGuidance::default(),
        pull_request_freshness: None,
        agent_session: None,
    }
}
