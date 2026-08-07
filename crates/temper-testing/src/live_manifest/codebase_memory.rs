use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jig_core::{Reply, RequestView, Script, ScriptFile};
use jig_server::FakeLlm;
use serde_json::Value as JsonValue;
use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{
    CiJobConclusion, IssueState, ItemNumber, PullRequest, PullRequestQuery, PullRequestState,
    RepositoryId, UserId,
};
use temper_workflow::{CiStatus, parse_metadata_block};
use toml::Value as TomlValue;

use super::convergence::{
    ci_observation_evidence, completed_ci_observation, issue_evidence, poll_until, pr_evidence,
    reject_labels, require_labels,
};
use super::{
    ENGINEER, FinalStateEvidence, ForcedSystemicFailureFixture, LiveCodebaseMemoryEvidence,
};

const MEMORY_FILE: &str = "src/lib.rs";
const MEMORY_RESULT_NEEDLE: &str = "FAKE_MCP_GRAPH_RESULT";
const ENGINEER_SUMMARY: &str =
    "Used codebase-memory graph evidence, then validated the retry-worker repair.";
const RAW_PROVIDER_FAILURE_NEEDLE: &str = "MCP-FIXTURE-SECRET";
const SAFE_PROVIDER_FAILURE: &str = "codebase-memory provider or protocol request failed; do not retry codebase-memory immediately; continue with read, grep, find, shell, or other conventional discovery instead";
const BOUNDED_GRAPH_RESULT_NEEDLE: &str = "[codebase-memory output truncated to 16384 bytes]";
const MAX_MODEL_MESSAGE_BYTES: usize = 20 * 1024;

pub(super) fn converge(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
    admin_user: &str,
    standalone: &mut super::process::ChildGuard,
    timeout: Duration,
    fake: &CodebaseMemoryFake,
    mcp: &FakeMcpServer,
) -> Result<(FinalStateEvidence, LiveCodebaseMemoryEvidence), String> {
    let final_state = drive_codebase_memory_convergence(
        forge, repository, issue, admin_user, standalone, timeout,
    )?;
    let calls = logged_tool_calls(&mcp.log_path)?;
    validate_mcp_contract(mcp, &calls)?;
    fake.validate_observations()?;
    if fake.engineer_requests() < 9 {
        return Err(format!(
            "fake LLM did not complete the codebase-memory readiness, fallback, and validation loop\n{}",
            fake.log_tail()
        ));
    }
    let mut mcp_call_counts = BTreeMap::<String, usize>::new();
    for call in &calls {
        *mcp_call_counts.entry(call.name.clone()).or_default() += 1;
    }
    let mcp_search_calls = mcp_call_counts
        .get("search_graph")
        .copied()
        .unwrap_or_default();
    Ok((
        final_state,
        LiveCodebaseMemoryEvidence {
            produced_file: MEMORY_FILE.to_string(),
            expected_result: MEMORY_RESULT_NEEDLE.to_string(),
            fake_mcp_log: mcp.log_path.clone(),
            mcp_search_calls,
            mcp_call_counts: mcp_call_counts.into_iter().collect(),
            readiness_delay_ms: mcp.readiness_delay_ms,
            forced_failure_tool: mcp
                .forced_systemic_failure
                .as_ref()
                .map(|failure| failure.tool.clone()),
            safe_tools: mcp
                .safe_tools
                .iter()
                .map(|tool| format!("codebase_memory_{tool}"))
                .collect(),
            hidden_tools: mcp
                .hidden_tools
                .iter()
                .map(|tool| format!("codebase_memory_{tool}"))
                .collect(),
            lifecycle: None,
        },
    ))
}

fn drive_codebase_memory_convergence(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
    admin_user: &str,
    standalone: &mut super::process::ChildGuard,
    timeout: Duration,
) -> Result<FinalStateEvidence, String> {
    let deadline = Instant::now() + timeout;
    match poll_until(deadline, standalone, || {
        super::process::engine_block_on(assert_codebase_memory_checkpoint(
            forge, repository, issue, admin_user,
        ))
    })? {
        CodebaseMemoryCheckpoint::OpenPr => poll_until(deadline, standalone, || {
            super::process::engine_block_on(assert_codebase_memory_converged(
                forge, repository, issue, admin_user,
            ))
        }),
        CodebaseMemoryCheckpoint::Converged(final_state) => Ok(final_state),
    }
}

enum CodebaseMemoryCheckpoint {
    OpenPr,
    Converged(FinalStateEvidence),
}

async fn assert_codebase_memory_checkpoint(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
    admin_user: &str,
) -> Result<CodebaseMemoryCheckpoint, String> {
    let mut errors = Vec::new();

    match assert_pr_open_with_memory_diff(forge, repository, issue).await {
        Ok(()) => return Ok(CodebaseMemoryCheckpoint::OpenPr),
        Err(error) => errors.push(("open implementation PR with memory diff", error)),
    }

    match assert_codebase_memory_converged(forge, repository, issue, admin_user).await {
        Ok(final_state) => Ok(CodebaseMemoryCheckpoint::Converged(final_state)),
        Err(error) => {
            errors.push(("final convergence", error));
            Err(format_codebase_memory_checkpoint_errors(&errors))
        }
    }
}

fn format_codebase_memory_checkpoint_errors(errors: &[(&'static str, String)]) -> String {
    let details = errors
        .iter()
        .map(|(phase, error)| format!("{phase}: {error}"))
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "codebase-memory workflow has not reached open implementation PR with memory diff or final convergence yet ({details})"
    )
}

async fn assert_pr_open_with_memory_diff(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
) -> Result<(), String> {
    let pr = implementation_pr(forge, repository, issue).await?;
    verify_engineer_pr(&pr, issue)?;
    if pr.state != PullRequestState::Open {
        return Err(format!(
            "implementation PR #{} is not open yet (state {:?})",
            pr.number, pr.state
        ));
    }
    require_labels(&pr.labels, &["implementation", "landing"])?;
    assert_pr_body_contains_engineer_summary(&pr)?;
    Ok(())
}

fn assert_pr_body_contains_engineer_summary(pr: &PullRequest) -> Result<(), String> {
    if !pr.body.contains(ENGINEER_SUMMARY) {
        return Err(format!(
            "implementation PR body does not contain engineer summary {:?}:\n{}",
            ENGINEER_SUMMARY, pr.body
        ));
    }
    Ok(())
}

async fn assert_codebase_memory_converged(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
    admin_user: &str,
) -> Result<FinalStateEvidence, String> {
    let pr = implementation_pr(forge, repository, issue).await?;
    verify_engineer_pr(&pr, issue)?;
    if pr.state != PullRequestState::Merged {
        return Err(format!(
            "implementation PR #{} is not merged yet (state {:?})",
            pr.number, pr.state
        ));
    }
    let merge = pr.merge.as_ref().ok_or("merged PR has no merge record")?;
    let expected_automation = [UserId::new(admin_user), UserId::new("bot")];
    if !expected_automation
        .iter()
        .any(|user| user == &merge.merged_by)
    {
        return Err(format!(
            "PR was merged by {:?}, expected automation identity {:?}",
            merge.merged_by, expected_automation
        ));
    }
    require_labels(&pr.labels, &["implementation"])?;
    reject_labels(&pr.labels, &["landing"])?;

    let ci_observation = completed_ci_observation(forge, repository, &pr).await?;
    let jobs = &ci_observation.jobs;
    if jobs.is_empty() {
        return Err(format!("no completed CI jobs for PR #{}", pr.number));
    }
    if jobs.last().and_then(|job| job.conclusion) != Some(CiJobConclusion::Success) {
        return Err(format!(
            "latest CI verdict for PR #{} is not success: {:?}",
            pr.number,
            jobs.last()
        ));
    }
    if !CiStatus::from_jobs(jobs).is_passed() {
        return Err("latest CI aggregate is not passing".to_string());
    }

    let issue = forge
        .get_issue_by_number(repository, issue)
        .await
        .map_err(|error| format!("source issue lookup failed: {error}"))?
        .ok_or("source issue disappeared")?;
    if issue.state != IssueState::Closed {
        return Err(format!(
            "source issue #{} not closed after merge (state {:?}, labels {:?})",
            issue.number, issue.state, issue.labels
        ));
    }
    require_labels(&issue.labels, &["code"])?;
    reject_labels(&issue.labels, &["untriaged", "ready", "in-progress"])?;

    Ok(FinalStateEvidence {
        issue: issue_evidence(&issue),
        pull_request: pr_evidence(&pr),
        ci_jobs: jobs
            .iter()
            .map(super::convergence::ci_job_evidence)
            .collect(),
        ci_observations: vec![ci_observation_evidence(&ci_observation)],
        ci_heads: Vec::new(),
    })
}

async fn implementation_pr(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
) -> Result<PullRequest, String> {
    let pull_requests: Vec<PullRequest> = forge
        .list_pull_requests(repository, PullRequestQuery::default())
        .await
        .map_err(|error| format!("list_pull_requests failed: {error}"))?
        .into_iter()
        .filter(|pr| pr.labels.iter().any(|label| label == "implementation"))
        .collect();
    if pull_requests.len() != 1 {
        return Err(format!(
            "expected exactly one implementation PR, found {}",
            pull_requests.len()
        ));
    }
    let pr = pull_requests.into_iter().next().expect("one PR");
    verify_metadata(&pr, issue)?;
    Ok(pr)
}

fn verify_engineer_pr(pr: &PullRequest, issue: ItemNumber) -> Result<(), String> {
    verify_metadata(pr, issue)?;
    if pr.author_id != UserId::new(ENGINEER) {
        return Err(format!(
            "implementation PR #{} authored by {:?}, not engineer {:?}",
            pr.number, pr.author_id, ENGINEER
        ));
    }
    Ok(())
}

fn verify_metadata(pr: &PullRequest, issue: ItemNumber) -> Result<(), String> {
    let metadata = parse_metadata_block(&pr.body)
        .map_err(|error| format!("implementation PR metadata is malformed: {error}"))?
        .ok_or("implementation PR is missing workflow metadata")?;
    let expected_key = format!("pr-for-code-{issue}");
    if metadata.correlation_key.as_deref() != Some(expected_key.as_str()) {
        return Err(format!(
            "implementation PR correlation key {:?} != {expected_key:?}",
            metadata.correlation_key
        ));
    }
    if !metadata
        .parents
        .iter()
        .any(|parent| parent.is_same_repo() && parent.number == issue)
    {
        return Err(format!(
            "implementation PR parents {:?} do not include issue #{issue}",
            metadata.parents
        ));
    }
    Ok(())
}

pub(super) struct ToolConfiguration {
    pub(super) role: String,
    pub(super) tool: String,
    pub(super) mode: String,
    pub(super) index: String,
    pub(super) tool_timeout_secs: Option<u64>,
}

pub(super) fn tune_codebase_memory_config(
    config_path: &Path,
    fake_mcp: &FakeMcpServer,
    configuration: &ToolConfiguration,
) -> Result<(), String> {
    let text = fs::read_to_string(config_path)
        .map_err(|error| format!("read {}: {error}", config_path.display()))?;
    let mut doc: TomlValue = text
        .parse()
        .map_err(|error| format!("parse {} as TOML: {error}", config_path.display()))?;
    let root = doc
        .as_table_mut()
        .ok_or_else(|| "config.toml root must be a table".to_string())?;
    let agent = root
        .entry("agent".to_string())
        .or_insert_with(|| TomlValue::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| "config.toml [agent] must be a table".to_string())?;
    if let Some(timeout) = configuration.tool_timeout_secs {
        let deadlines = agent
            .entry("deadlines".to_string())
            .or_insert_with(|| TomlValue::Table(Default::default()))
            .as_table_mut()
            .ok_or_else(|| "config.toml [agent.deadlines] must be a table".to_string())?;
        deadlines.insert(
            "tool_timeout_secs".to_string(),
            TomlValue::Integer(i64::try_from(timeout).expect("bounded timeout fits i64")),
        );
    }
    let tools = agent
        .entry("tools".to_string())
        .or_insert_with(|| TomlValue::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| "config.toml [agent.tools] must be a table".to_string())?;
    let mut codebase = toml::map::Map::new();
    codebase.insert(
        "mode".to_string(),
        TomlValue::String(configuration.mode.clone()),
    );
    codebase.insert(
        "command".to_string(),
        TomlValue::String("python3".to_string()),
    );
    codebase.insert(
        "args".to_string(),
        TomlValue::Array(vec![
            TomlValue::String("-u".to_string()),
            TomlValue::String(fake_mcp.script_path.display().to_string()),
            TomlValue::String(fake_mcp.log_path.display().to_string()),
            TomlValue::String("demo".to_string()),
            TomlValue::String(fake_mcp.project.clone()),
            TomlValue::String(
                serde_json::to_string(&fake_mcp.safe_tools)
                    .map_err(|error| format!("serialize declared safe MCP tools: {error}"))?,
            ),
            TomlValue::String(
                serde_json::to_string(&fake_mcp.hidden_tools)
                    .map_err(|error| format!("serialize declared hidden MCP tools: {error}"))?,
            ),
            TomlValue::String(fake_mcp.readiness_delay_ms.to_string()),
            TomlValue::String(
                fake_mcp
                    .forced_systemic_failure
                    .as_ref()
                    .map(|failure| failure.tool.as_str())
                    .unwrap_or("")
                    .to_string(),
            ),
            TomlValue::String(
                fake_mcp
                    .forced_systemic_failure
                    .as_ref()
                    .map(|failure| failure.after_calls)
                    .unwrap_or_default()
                    .to_string(),
            ),
        ]),
    );
    codebase.insert(
        "roles".to_string(),
        TomlValue::Array(vec![TomlValue::String(configuration.role.clone())]),
    );
    codebase.insert(
        "index".to_string(),
        TomlValue::String(configuration.index.clone()),
    );
    codebase.insert("startup_timeout_secs".to_string(), TomlValue::Integer(2));
    codebase.insert("index_timeout_secs".to_string(), TomlValue::Integer(3));
    tools.insert(configuration.tool.clone(), TomlValue::Table(codebase));
    fs::write(
        config_path,
        toml::to_string_pretty(&doc).map_err(|error| format!("serialize tuned config: {error}"))?,
    )
    .map_err(|error| format!("write tuned config {}: {error}", config_path.display()))
}

pub(super) struct FakeMcpServer {
    pub(super) script_path: PathBuf,
    pub(super) log_path: PathBuf,
    pub(super) project: String,
    pub(super) safe_tools: Vec<String>,
    pub(super) hidden_tools: Vec<String>,
    pub(super) readiness_delay_ms: u64,
    pub(super) forced_systemic_failure: Option<ForcedSystemicFailureFixture>,
}

pub(super) fn write_fake_mcp(
    root: &Path,
    project: &str,
    safe_tools: &[String],
    hidden_tools: &[String],
    readiness_delay_ms: u64,
    forced_systemic_failure: Option<&ForcedSystemicFailureFixture>,
) -> Result<FakeMcpServer, String> {
    let script_path = root.join("fake-codebase-memory-mcp.py");
    let log_path = root.join("logs/fake-codebase-memory-mcp.jsonl");
    fs::write(&script_path, FAKE_MCP_SCRIPT)
        .map_err(|error| format!("write fake MCP server {}: {error}", script_path.display()))?;
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create fake MCP log dir {}: {error}", parent.display()))?;
    }
    fs::write(&log_path, "")
        .map_err(|error| format!("create fake MCP log {}: {error}", log_path.display()))?;
    Ok(FakeMcpServer {
        script_path,
        log_path,
        project: project.to_string(),
        safe_tools: safe_tools.to_vec(),
        hidden_tools: hidden_tools.to_vec(),
        readiness_delay_ms,
        forced_systemic_failure: forced_systemic_failure.cloned(),
    })
}

fn validate_mcp_contract(mcp: &FakeMcpServer, calls: &[McpToolCallEvidence]) -> Result<(), String> {
    let failure = mcp.forced_systemic_failure.as_ref();
    let graph_tool = failure
        .map(|failure| failure.tool.as_str())
        .unwrap_or("search_graph");
    let expected_graph_calls = failure.map(|failure| failure.after_calls + 1).unwrap_or(1);
    let graph = calls
        .iter()
        .enumerate()
        .filter(|(_, call)| call.name == graph_tool)
        .collect::<Vec<_>>();
    if graph.len() != expected_graph_calls {
        return Err(format!(
            "expected {expected_graph_calls} {graph_tool} MCP call(s), found {} in {}",
            graph.len(),
            mcp.log_path.display()
        ));
    }
    if failure.is_some()
        && (graph[..graph.len().saturating_sub(1)]
            .iter()
            .any(|(_, call)| call.is_error)
            || graph.last().is_some_and(|(_, call)| !call.is_error))
    {
        return Err(format!(
            "{graph_tool} did not preserve one successful graph result before the controlled systemic failure: {graph:?}"
        ));
    }
    let index = calls
        .iter()
        .filter(|call| call.name == "index_repository")
        .collect::<Vec<_>>();
    if index.len() != 1
        || index[0].arguments.get("repo_path").is_none()
        || index[0].arguments.get("name").is_none()
    {
        return Err(format!(
            "index_repository was not exercised exactly once internally with repo_path and stable name: {calls:?}"
        ));
    }
    let provider_project = index[0]
        .arguments
        .get("name")
        .and_then(JsonValue::as_str)
        .filter(|name| name.starts_with("temper-v1-"))
        .ok_or_else(|| {
            format!(
                "index_repository did not receive a stable provider project identity: {index:?}"
            )
        })?;
    if graph.iter().any(|(_, call)| {
        call.arguments.get("project").and_then(JsonValue::as_str) != Some(provider_project)
    }) {
        return Err(format!(
            "{graph_tool} did not translate the workspace alias to stable provider project {provider_project}: {graph:?}"
        ));
    }
    if index[0].delay_ms != Some(mcp.readiness_delay_ms) {
        return Err(format!(
            "background index delay was not retained as {}ms: {index:?}",
            mcp.readiness_delay_ms
        ));
    }
    let status = calls
        .iter()
        .filter(|call| call.name == "index_status")
        .collect::<Vec<_>>();
    if status.len() != 1
        || status[0]
            .arguments
            .get("project")
            .and_then(JsonValue::as_str)
            != Some(provider_project)
    {
        return Err(format!(
            "expected one targeted index_status discovery call for stable provider project {provider_project}, found {status:?}"
        ));
    }
    let expected_counts = BTreeMap::from([
        ("index_repository".to_string(), 1_usize),
        ("index_status".to_string(), 1_usize),
        (graph_tool.to_string(), expected_graph_calls),
    ]);
    let mut actual_counts = BTreeMap::<String, usize>::new();
    for call in calls {
        *actual_counts.entry(call.name.clone()).or_default() += 1;
    }
    if actual_counts != expected_counts {
        return Err(format!(
            "unexpected MCP request inventory; expected {expected_counts:?}, got {actual_counts:?}"
        ));
    }
    if let Some((failure_position, _)) = graph.last() {
        if calls.len() != failure_position + 1 {
            return Err(format!(
                "a codebase-memory MCP call followed the controlled systemic failure: {calls:?}"
            ));
        }
    }
    Ok(())
}

#[derive(Debug)]
struct McpToolCallEvidence {
    name: String,
    arguments: JsonValue,
    delay_ms: Option<u64>,
    is_error: bool,
}

fn logged_tool_calls(path: &Path) -> Result<Vec<McpToolCallEvidence>, String> {
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("read MCP call log {}: {error}", path.display()))?;
    Ok(raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let value = serde_json::from_str::<JsonValue>(line).ok()?;
            let name = value.get("tool")?.as_str()?.to_string();
            let arguments = value.get("arguments").cloned().unwrap_or(JsonValue::Null);
            let delay_ms = value.get("delay_ms").and_then(JsonValue::as_u64);
            let is_error = value
                .get("is_error")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            Some(McpToolCallEvidence {
                name,
                arguments,
                delay_ms,
                is_error,
            })
        })
        .collect())
}

pub(super) struct CodebaseMemoryFake {
    fake: FakeLlm,
    engineer_requests: Arc<AtomicUsize>,
    observations: Arc<Mutex<ModelObservations>>,
}

#[derive(Default)]
struct ModelObservations {
    prompt_guidance_seen: bool,
    memory_result_seen: bool,
    safe_failure_seen: bool,
    raw_provider_text_seen: bool,
    bounded_graph_result_seen: bool,
    oversized_message_seen: bool,
}

impl CodebaseMemoryFake {
    pub(super) fn start(script_path: &Path) -> Result<Self, String> {
        let script = ScriptFile::load(script_path)
            .map_err(|error| {
                format!(
                    "load scenario Jig script {}: {error}",
                    script_path.display()
                )
            })?
            .into_script();
        let engineer_requests = Arc::new(AtomicUsize::new(0));
        let request_count = Arc::clone(&engineer_requests);
        let observations = Arc::new(Mutex::new(ModelObservations::default()));
        let observations_for_rule = Arc::clone(&observations);
        let fake = FakeLlm::start(Script::rule(move |view| {
            if !messages_contain(view, "ROLE: engineer") {
                return Reply::text("unexpected codebase-memory fake-LLM request");
            }
            request_count.fetch_add(1, Ordering::SeqCst);
            let mut observations = observations_for_rule.lock().expect("observations lock");
            if messages_contain(view, "CODEBASE MEMORY") {
                observations.prompt_guidance_seen = true;
            }
            if messages_contain(view, MEMORY_RESULT_NEEDLE) {
                observations.memory_result_seen = true;
            }
            if messages_contain(view, SAFE_PROVIDER_FAILURE) {
                observations.safe_failure_seen = true;
            }
            if messages_contain(view, RAW_PROVIDER_FAILURE_NEEDLE) {
                observations.raw_provider_text_seen = true;
            }
            if messages_contain(view, BOUNDED_GRAPH_RESULT_NEEDLE) {
                observations.bounded_graph_result_seen = true;
            }
            if view
                .messages
                .iter()
                .any(|message| message.content.len() > MAX_MODEL_MESSAGE_BYTES)
            {
                observations.oversized_message_seen = true;
            }
            drop(observations);
            script.next_reply(view)
        }))
        .map_err(|error| format!("start scenario Jig fake LLM: {error}"))?;
        Ok(Self {
            fake,
            engineer_requests,
            observations,
        })
    }

    pub(super) fn base_url(&self) -> String {
        self.fake.base_url()
    }

    pub(super) fn engineer_requests(&self) -> usize {
        self.engineer_requests.load(Ordering::SeqCst)
    }

    fn validate_observations(&self) -> Result<(), String> {
        let (
            prompt_guidance_seen,
            memory_result_seen,
            safe_failure_seen,
            raw_provider_text_seen,
            bounded_graph_result_seen,
            oversized_message_seen,
        ) = {
            let observations = self
                .observations
                .lock()
                .map_err(|_| "model observation mutex poisoned".to_string())?;
            (
                observations.prompt_guidance_seen,
                observations.memory_result_seen,
                observations.safe_failure_seen,
                observations.raw_provider_text_seen,
                observations.bounded_graph_result_seen,
                observations.oversized_message_seen,
            )
        };
        if !prompt_guidance_seen {
            return Err(format!(
                "fake LLM did not receive CODEBASE MEMORY prompt guidance\n{}",
                self.log_tail()
            ));
        }
        if !memory_result_seen {
            return Err(format!(
                "fake LLM did not receive the fake MCP graph result\n{}",
                self.log_tail()
            ));
        }
        if !bounded_graph_result_seen {
            return Err(format!(
                "fake LLM did not receive the bounded graph result marker\n{}",
                self.log_tail()
            ));
        }
        if !safe_failure_seen {
            return Err(format!(
                "fake LLM did not receive the bounded typed systemic diagnostic\n{}",
                self.log_tail()
            ));
        }
        if raw_provider_text_seen {
            return Err(format!(
                "raw provider failure text leaked into the fake LLM request\n{}",
                self.log_tail()
            ));
        }
        if oversized_message_seen {
            return Err(format!(
                "a model-visible message exceeded the scenario's bounded result allowance\n{}",
                self.log_tail()
            ));
        }
        Ok(())
    }

    pub(super) fn log_tail(&self) -> String {
        let requests = self.fake.requests();
        let observations = self.observations.lock().expect("observations lock");
        let mut lines = vec![format!(
            "observations: prompt_guidance_seen={} memory_result_seen={} bounded_graph_result_seen={} safe_failure_seen={} raw_provider_text_seen={} oversized_message_seen={}",
            observations.prompt_guidance_seen,
            observations.memory_result_seen,
            observations.bounded_graph_result_seen,
            observations.safe_failure_seen,
            observations.raw_provider_text_seen,
            observations.oversized_message_seen,
        )];
        if requests.is_empty() {
            lines.push("<fake LLM received no requests>".to_string());
            return lines.join("\n");
        }
        let start = requests.len().saturating_sub(20);
        lines.extend(
            requests[start..]
                .iter()
                .enumerate()
                .map(|(offset, request)| {
                    let index = start + offset + 1;
                    let view = request.view.as_ref();
                    let prior = view.map(|v| v.prior_tool_results).unwrap_or_default();
                    let last = view
                        .and_then(RequestView::last_message)
                        .map(|m| format!("{}: {}", m.role, snippet(&m.content, 160)))
                        .unwrap_or_else(|| "<no projected message>".to_string());
                    format!(
                        "#{index} {} {} role=engineer prior_tool_results={prior} last={last}",
                        request.method, request.path
                    )
                }),
        );
        lines.join("\n")
    }
}

fn messages_contain(view: &RequestView, needle: &str) -> bool {
    view.messages
        .iter()
        .any(|message| message.content.contains(needle))
}

fn snippet(text: &str, max: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= max {
            out.push('…');
            break;
        }
        out.push(if ch == '\n' { ' ' } else { ch });
    }
    out
}

const FAKE_MCP_SCRIPT: &str = include_str!("fake_codebase_memory_mcp.py");
