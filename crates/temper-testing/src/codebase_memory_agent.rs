use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use jig_core::{Reply, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::Value;
use temper_agent::{
    ProviderConfig, WorkspaceContext, WorkspaceGuidance, WorkspaceRepository, WorkspaceWorkItem,
    run_coding_agent_native_with_tool_config,
};
use temper_protocol_agent::{
    AgentToolConfig, CodebaseMemoryIndex, CodebaseMemoryMode, CodebaseMemoryToolConfig,
};

mod fake_mcp;
mod validation;
mod workspace;
pub use fake_mcp::McpToolCallEvidence;

const REPO_DIR: &str = "demo";
const REPO_SLUG: &str = "acme/demo";
const ACTUAL_PROJECT: &str = "actual-demo-project";
const MEMORY_QUERY: &str = "WidgetService";
const MEMORY_FILE: &str = "MEMORY_NOTES.md";

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CodebaseMemoryAgentScenarioEvidence {
    pub repo_slug: String,
    pub repo_root: PathBuf,
    pub actual_project: String,
    pub tool_config_enabled: bool,
    pub model_tool_names: Vec<String>,
    pub mcp_tool_calls: Vec<McpToolCallEvidence>,
    pub prompt_guidance_seen: bool,
    pub memory_result_seen_by_model: bool,
    pub final_summary: String,
    pub produced_file: PathBuf,
    pub produced_file_content: String,
    pub fake_llm_requests: usize,
}

#[derive(Default)]
struct ModelObservations {
    prompt_guidance_seen: bool,
    memory_result_seen: bool,
}

pub fn run_codebase_memory_agent_scenario() -> Result<CodebaseMemoryAgentScenarioEvidence, String> {
    let temp = tempfile::tempdir().map_err(|error| format!("create temp dir: {error}"))?;
    let workspace = temp.path().join("workspace");
    let repo_root = workspace.join(REPO_DIR);
    fs::create_dir_all(&repo_root)
        .map_err(|error| format!("create repo dir {}: {error}", repo_root.display()))?;
    workspace::initialise_git_repo(&repo_root)?;
    let repo_root = repo_root
        .canonicalize()
        .map_err(|error| format!("canonicalize repo root: {error}"))?;
    let workspace = workspace
        .canonicalize()
        .map_err(|error| format!("canonicalize workspace root: {error}"))?;

    let mcp = fake_mcp::write_fake_server(temp.path())?;
    let observations = Arc::new(Mutex::new(ModelObservations::default()));
    let fake = codebase_memory_agent_fake(Arc::clone(&observations));
    let provider = ProviderConfig::new(
        "jig-openai-compatible",
        "codebase-memory-agent-scenario",
        "https://example.invalid/unused-production-url",
        "sk-jig-test",
    )
    .with_base_url_override(fake.base_url());
    let tool_config = codebase_memory_tool_config(&mcp.script_path, &mcp.log_path, &repo_root);
    let context = workspace_context();

    let result = temper_engine_io::block_on_with({
        let context = context.clone();
        let workspace = workspace.clone();
        move |_cx, handle| async move {
            run_coding_agent_native_with_tool_config(
                handle,
                &provider,
                &context,
                &workspace,
                8,
                None,
                Some(&tool_config),
            )
            .await
        }
    })
    .map_err(|error| error.to_string())?;

    let produced_file = repo_root.join(MEMORY_FILE);
    let produced_file_content = fs::read_to_string(&produced_file)
        .map_err(|error| format!("read produced file {}: {error}", produced_file.display()))?;
    let mcp_tool_calls = fake_mcp::logged_tool_calls(&mcp.log_path)?;
    let model_tool_names = model_tool_names(&fake.requests())?;
    let observations = observations
        .lock()
        .map_err(|_| "model observation mutex poisoned".to_string())?;

    let evidence = CodebaseMemoryAgentScenarioEvidence {
        repo_slug: REPO_SLUG.to_string(),
        repo_root,
        actual_project: ACTUAL_PROJECT.to_string(),
        tool_config_enabled: true,
        model_tool_names,
        mcp_tool_calls,
        prompt_guidance_seen: observations.prompt_guidance_seen,
        memory_result_seen_by_model: observations.memory_result_seen,
        final_summary: result.summary.unwrap_or_default(),
        produced_file,
        produced_file_content,
        fake_llm_requests: fake.requests().len(),
    };
    validation::validate_evidence(&evidence, ACTUAL_PROJECT)?;
    Ok(evidence)
}

fn codebase_memory_agent_fake(observations: Arc<Mutex<ModelObservations>>) -> FakeLlm {
    FakeLlm::start(Script::rule(move |view| match view.prior_tool_results {
        0 => {
            if messages_contain(view, "CODEBASE MEMORY") {
                observations
                    .lock()
                    .expect("observations lock")
                    .prompt_guidance_seen = true;
            }
            Reply {
                turns: vec![Turn::ToolCall {
                    id: "call_memory_search".to_string(),
                    name: "codebase_memory_search_code".to_string(),
                    args: serde_json::json!({ "query": MEMORY_QUERY }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            }
        }
        1 => {
            let memory_line = memory_result_line(view).unwrap_or_else(|| "missing memory".into());
            if memory_line.contains("FAKE_MCP_SEARCH_RESULT") {
                observations
                    .lock()
                    .expect("observations lock")
                    .memory_result_seen = true;
            }
            Reply {
                turns: vec![Turn::ToolCall {
                    id: "call_write_memory_notes".to_string(),
                    name: "write".to_string(),
                    args: serde_json::json!({
                        "path": format!("{REPO_DIR}/{MEMORY_FILE}"),
                        "content": format!("memory-guided notes\n{memory_line}\n"),
                    }),
                }],
                usage: Default::default(),
                stop: StopReason::ToolCalls,
            }
        }
        _ => Reply::text(
            serde_json::json!({
                "summary": "Used codebase memory search result before writing MEMORY_NOTES.md."
            })
            .to_string(),
        ),
    }))
    .expect("start codebase-memory fake LLM")
}

fn messages_contain(view: &jig_core::RequestView, needle: &str) -> bool {
    view.messages
        .iter()
        .any(|message| message.content.contains(needle))
}

fn memory_result_line(view: &jig_core::RequestView) -> Option<String> {
    view.messages
        .iter()
        .filter(|message| message.role == "tool")
        .flat_map(|message| message.content.lines())
        .find(|line| line.contains("FAKE_MCP_SEARCH_RESULT"))
        .map(str::to_string)
}

fn codebase_memory_tool_config(
    script_path: &Path,
    log_path: &Path,
    repo_root: &Path,
) -> AgentToolConfig {
    AgentToolConfig {
        codebase_memory: Some(CodebaseMemoryToolConfig {
            mode: CodebaseMemoryMode::Required,
            command: "python3".to_string(),
            args: vec![
                "-u".to_string(),
                script_path.display().to_string(),
                log_path.display().to_string(),
                repo_root.display().to_string(),
                ACTUAL_PROJECT.to_string(),
            ],
            roles: vec!["engineer".to_string()],
            index: CodebaseMemoryIndex::Blocking,
            startup_timeout_secs: 2,
            index_timeout_secs: 3,
        }),
    }
}

fn workspace_context() -> WorkspaceContext {
    WorkspaceContext {
        repos: vec![WorkspaceRepository {
            id: "repo-1".to_string(),
            owner: "acme".to_string(),
            name: "demo".to_string(),
            default_branch: "main".to_string(),
            dir: REPO_DIR.to_string(),
            access: "writable".to_string(),
            base_branch: "main".to_string(),
            branch_hint: Some("agent/pr-for-codebase-memory-agent".to_string()),
        }],
        work_item: WorkspaceWorkItem {
            role: "engineer".to_string(),
            queue: "code_ready".to_string(),
            kind: "code".to_string(),
            target: "Issue { number: ItemNumber(82) }".to_string(),
            context: serde_json::json!({
                "artifact": {
                    "type": "issue",
                    "number": 82,
                    "title": "Validate codebase-memory agent path",
                    "body": "Use codebase memory, then write MEMORY_NOTES.md.",
                    "labels": ["code", "ready"],
                    "state": "Open"
                }
            })
            .to_string(),
        },
        action: "open_pr".to_string(),
        correlation_key: "pr-for-codebase-memory-agent".to_string(),
        checkout: Some("writable".to_string()),
        allowed_verdicts: vec!["needs_architect".to_string()],
        guidance: WorkspaceGuidance {
            role_guidance: Some("Use codebase memory before editing.".to_string()),
            tool_guidance: Some("Use the workspace tools to edit files.".to_string()),
            tool_constraints: vec!["Do not run git commit.".to_string()],
        },
        pull_request_freshness: None,
        agent_session: None,
    }
}

fn model_tool_names(requests: &[jig_core::RecordedRequest]) -> Result<Vec<String>, String> {
    let mut names = BTreeSet::new();
    for request in requests {
        let value = serde_json::from_slice::<Value>(&request.body)
            .map_err(|error| format!("parse fake LLM request body: {error}"))?;
        collect_model_tool_names(&value, &mut names);
    }
    Ok(names.into_iter().collect())
}

fn collect_model_tool_names(value: &Value, names: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if let Some(tools) = object.get("tools").and_then(Value::as_array) {
                for tool in tools {
                    if let Some(name) = tool
                        .get("function")
                        .and_then(|function| function.get("name"))
                        .or_else(|| tool.get("name"))
                        .and_then(Value::as_str)
                    {
                        names.insert(name.to_string());
                    }
                }
            }
            for child in object.values() {
                collect_model_tool_names(child, names);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_model_tool_names(child, names);
            }
        }
        _ => {}
    }
}
