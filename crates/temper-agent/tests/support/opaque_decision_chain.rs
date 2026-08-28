//! Native-Jig coverage for opaque result-derived graph decision chains.

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
#[path = "opaque_decision_chain/context.rs"]
mod context;
use context::workspace_context;
#[path = "opaque_decision_chain/guidance.rs"]
mod guidance;
use guidance::assert_guidance;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecisionCase {
    Consumed,
    StagedDetourRecovery,
    UnrelatedLaterTarget,
    ProducerTurnDependents,
    ImplementationOnlyProviderFallback,
    ImplementationFocusedProviderFallback,
    NoRetainedDecisionEvidence,
    ConventionalReadSubstitution,
    IncompleteSourceEvidence,
    UnavailableAfterRoot,
    UnconsumableRecoveryExhausted,
    AllRootsNonViableFallback,
    SelectorlessViableRootFallback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecisionStep {
    Discovery,
    Refinement,
    Trace,
    ImplementationSource,
    FocusedTestTraversal,
    CallerSource,
    CallerSourceDetour,
    BehavioralTestSource,
    FocusedTestDetour,
    Mutation,
    MutationAttempt,
    MutationBlocked,
    UnrelatedLaterTarget,
    ProducerTurnDependents,
    UnavailableFallback,
    ProviderFailure,
    Recovery,
    GraphRetry,
    ConventionalDiscovery,
    SourceRead,
    Validation,
    Submission,
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
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let checkout = TempCheckout::new("jig-opaque-decision-chain");
    checkout.init_git();
    if matches!(
        case,
        DecisionCase::Consumed
            | DecisionCase::StagedDetourRecovery
            | DecisionCase::UnavailableAfterRoot
            | DecisionCase::AllRootsNonViableFallback
            | DecisionCase::ImplementationOnlyProviderFallback
            | DecisionCase::ImplementationFocusedProviderFallback
            | DecisionCase::SelectorlessViableRootFallback
    ) {
        fs::write(
            checkout.repo_path().join("EVIDENCE.md"),
            "pending exact read\n",
        )
        .expect("seed exact-read evidence target");
        checkout.git(&["add", "EVIDENCE.md"]);
        checkout.git(&["commit", "-m", "seed exact-read evidence target"]);
    }
    if case == DecisionCase::ImplementationOnlyProviderFallback {
        fs::write(
            checkout.repo_path().join("FALLBACK.md"),
            "pending independent fallback\n",
        )
        .expect("seed independent conventional fallback target");
        checkout.git(&["add", "FALLBACK.md"]);
        checkout.git(&["commit", "-m", "seed independent fallback target"]);
    }

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
            20,
            None,
            Some(&tool_config),
        )
        .await
    });
    match (case, result) {
        (
            DecisionCase::Consumed
            | DecisionCase::StagedDetourRecovery
            | DecisionCase::UnavailableAfterRoot
            | DecisionCase::ImplementationOnlyProviderFallback
            | DecisionCase::ImplementationFocusedProviderFallback
            | DecisionCase::AllRootsNonViableFallback
            | DecisionCase::SelectorlessViableRootFallback,
            Ok(result),
        ) => {
            assert_eq!(result.verdict, None)
        }
        (
            DecisionCase::Consumed
            | DecisionCase::StagedDetourRecovery
            | DecisionCase::UnavailableAfterRoot
            | DecisionCase::ImplementationOnlyProviderFallback
            | DecisionCase::ImplementationFocusedProviderFallback
            | DecisionCase::AllRootsNonViableFallback
            | DecisionCase::SelectorlessViableRootFallback,
            Err(error),
        ) => {
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
        mutation: fs::read_to_string(checkout.repo_path().join(
            if case == DecisionCase::ImplementationOnlyProviderFallback {
                "FALLBACK.md"
            } else {
                "EVIDENCE.md"
            },
        ))
        .ok(),
    }
}

#[path = "opaque_decision_chain/fake.rs"]
mod fake;
use fake::decision_chain_fake;

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
