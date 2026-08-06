use std::collections::BTreeSet;
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
use temper_protocol_agent::CodebaseMemoryRetentionPolicy;
use temper_worker::{
    CodebaseMemoryMaintenanceConfig, CodebaseMemoryRecoveryMode, CodebaseMemoryRetentionScope,
    run_codebase_memory_recovery,
};
use temper_workflow::{CiStatus, parse_metadata_block};
use toml::Value as TomlValue;

use super::convergence::{
    ci_observation_evidence, completed_ci_observation, issue_evidence, poll_until, pr_evidence,
    reject_labels, require_labels,
};
use super::{ENGINEER, FinalStateEvidence, LiveCodebaseMemoryEvidence};

const MEMORY_FILE: &str = "MEMORY_NOTES.md";
const MEMORY_RESULT_NEEDLE: &str = "FAKE_MCP_SEARCH_RESULT";
const ENGINEER_SUMMARY: &str = "Used codebase memory search result before writing MEMORY_NOTES.md.";

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
    validate_mcp_contract(mcp)?;
    fake.validate_observations()?;
    let lifecycle = validate_lifecycle(mcp)?;
    if fake.engineer_requests() < 4 {
        return Err(format!(
            "fake LLM did not complete the codebase-memory engineer tool loop\n{}",
            fake.log_tail()
        ));
    }
    let search_calls = logged_tool_calls(&mcp.log_path)?
        .iter()
        .filter(|call| call.name == "search_code")
        .count();
    Ok((
        final_state,
        LiveCodebaseMemoryEvidence {
            produced_file: MEMORY_FILE.to_string(),
            expected_result: MEMORY_RESULT_NEEDLE.to_string(),
            fake_mcp_log: mcp.log_path.clone(),
            mcp_search_calls: search_calls,
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
            lifecycle,
        },
    ))
}

fn validate_lifecycle(mcp: &FakeMcpServer) -> Result<Option<String>, String> {
    if !mcp.lifecycle_fixture {
        return Ok(None);
    }
    let scope = CodebaseMemoryRetentionScope {
        workspace_root: mcp.workspace_root.join("workspaces"),
        roles: BTreeSet::from(["engineer".to_string()]),
        repository_dirs: BTreeSet::from(["demo".to_string()]),
    };
    fs::create_dir_all(&scope.workspace_root)
        .map_err(|error| format!("create lifecycle workspace root: {error}"))?;
    let policy = CodebaseMemoryRetentionPolicy {
        enabled: true,
        max_obsolete_projects: 64,
        max_age_days: 30,
        maintenance_interval_secs: 3600,
        maintenance_timeout_secs: 20,
        inventory_page_size: 200,
        max_inventory_pages: 4,
        max_deletions_per_run: 100,
    };
    let config = CodebaseMemoryMaintenanceConfig::new(
        "python3",
        mcp.provider_args(),
        Duration::from_secs(2),
        Duration::from_secs(3),
        policy,
        scope,
    );
    let dry = run_codebase_memory_recovery(
        &config,
        CodebaseMemoryRecoveryMode::DryRun,
        None,
        None,
        &|| false,
    );
    if dry.retention.proposed.is_empty() || !dry.retention.deleted.is_empty() {
        return Err(format!("lifecycle dry-run was not deletion-free: {dry:?}"));
    }
    let mut failed_seen = false;
    for _ in 0..4 {
        let review = run_codebase_memory_recovery(
            &config,
            CodebaseMemoryRecoveryMode::DryRun,
            None,
            None,
            &|| false,
        );
        let plan = review
            .plan_id
            .clone()
            .ok_or("lifecycle dry-run omitted plan id")?;
        let applied = run_codebase_memory_recovery(
            &config,
            CodebaseMemoryRecoveryMode::Apply,
            Some(&plan),
            None,
            &|| false,
        );
        failed_seen |= !applied.retention.failed.is_empty();
        if applied.retention.proposed.len() <= 1 {
            break;
        }
    }
    let state: JsonValue = serde_json::from_str(
        &fs::read_to_string(&mcp.state_path)
            .map_err(|error| format!("read lifecycle provider state: {error}"))?,
    )
    .map_err(|error| format!("parse lifecycle provider state: {error}"))?;
    let projects = state["projects"]
        .as_object()
        .ok_or("lifecycle state omitted projects")?;
    for protected in [
        "temper-v1-protected",
        "unrelated-project",
        "ambiguous-project",
        "active-project",
    ] {
        if !projects.contains_key(protected) {
            return Err(format!(
                "lifecycle cleanup deleted protected record {protected}"
            ));
        }
    }
    if !failed_seen || !projects.contains_key("legacy-temper-000") {
        return Err("lifecycle cleanup did not isolate the injected deletion failure".to_string());
    }
    let obsolete = projects
        .keys()
        .filter(|name| name.starts_with("legacy-temper-"))
        .count();
    if obsolete > 64 {
        return Err(format!(
            "lifecycle cleanup retained {obsolete} obsolete projects, bound is 64"
        ));
    }
    Ok(Some(format!(
        "dry_run_candidates={} final_projects={} obsolete={} cache_bytes={} partial_failure=true protected=stable,active,unrelated,ambiguous",
        dry.retention.proposed.len(),
        projects.len(),
        obsolete,
        state["cache_bytes"].as_u64().unwrap_or_default()
    )))
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
            TomlValue::String("--state".to_string()),
            TomlValue::String(fake_mcp.state_path.display().to_string()),
            TomlValue::String("--log".to_string()),
            TomlValue::String(fake_mcp.log_path.display().to_string()),
            TomlValue::String("--mode".to_string()),
            TomlValue::String("stateful".to_string()),
            TomlValue::String("--seed-json".to_string()),
            TomlValue::String(fake_mcp.seed_json.clone()),
            TomlValue::String("--safe-tools-json".to_string()),
            TomlValue::String(
                serde_json::to_string(&fake_mcp.safe_tools)
                    .map_err(|error| format!("serialize declared safe MCP tools: {error}"))?,
            ),
            TomlValue::String("--hidden-tools-json".to_string()),
            TomlValue::String(
                serde_json::to_string(&fake_mcp.hidden_tools)
                    .map_err(|error| format!("serialize declared hidden MCP tools: {error}"))?,
            ),
            TomlValue::String("--delay-list-ms".to_string()),
            TomlValue::String(
                if fake_mcp.lifecycle_fixture {
                    "4000"
                } else {
                    "0"
                }
                .to_string(),
            ),
            TomlValue::String("--fail-tools-json".to_string()),
            TomlValue::String(
                if fake_mcp.lifecycle_fixture {
                    r#"{"delete_project":["legacy-temper-000"]}"#
                } else {
                    "{}"
                }
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
    pub(super) state_path: PathBuf,
    pub(super) log_path: PathBuf,
    pub(super) safe_tools: Vec<String>,
    pub(super) hidden_tools: Vec<String>,
    lifecycle_fixture: bool,
    seed_json: String,
    workspace_root: PathBuf,
}

impl FakeMcpServer {
    fn provider_args(&self) -> Vec<String> {
        vec![
            "-u".into(),
            self.script_path.display().to_string(),
            "--state".into(),
            self.state_path.display().to_string(),
            "--log".into(),
            self.log_path.display().to_string(),
            "--mode".into(),
            "stateful".into(),
            "--seed-json".into(),
            self.seed_json.clone(),
            "--safe-tools-json".into(),
            serde_json::to_string(&self.safe_tools).expect("tool names serialize"),
            "--hidden-tools-json".into(),
            serde_json::to_string(&self.hidden_tools).expect("tool names serialize"),
            "--delay-list-ms".into(),
            if self.lifecycle_fixture { "250" } else { "0" }.into(),
            "--fail-tools-json".into(),
            if self.lifecycle_fixture {
                r#"{"delete_project":["legacy-temper-000"]}"#
            } else {
                "{}"
            }
            .into(),
        ]
    }
}

pub(super) fn write_fake_mcp(
    root: &Path,
    _project: &str,
    fixture: Option<&str>,
    safe_tools: &[String],
    hidden_tools: &[String],
) -> Result<FakeMcpServer, String> {
    let script_path = root.join("fake-codebase-memory-mcp.py");
    let state_path = root.join("fake-codebase-memory-state.json");
    let log_path = root.join("logs/fake-codebase-memory-mcp.jsonl");
    fs::write(&script_path, FAKE_MCP_SCRIPT)
        .map_err(|error| format!("write fake MCP server {}: {error}", script_path.display()))?;
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create fake MCP log dir {}: {error}", parent.display()))?;
    }
    fs::write(&log_path, "")
        .map_err(|error| format!("create fake MCP log {}: {error}", log_path.display()))?;
    let lifecycle_fixture = fixture == Some("stable-lifecycle");
    let seed_json = if lifecycle_fixture {
        serde_json::to_string(&lifecycle_seed(root))
            .map_err(|error| format!("serialize lifecycle MCP seed: {error}"))?
    } else {
        "{}".to_string()
    };
    Ok(FakeMcpServer {
        script_path,
        state_path,
        log_path,
        safe_tools: safe_tools.to_vec(),
        hidden_tools: hidden_tools.to_vec(),
        lifecycle_fixture,
        seed_json,
        workspace_root: root.to_path_buf(),
    })
}

fn lifecycle_seed(root: &Path) -> JsonValue {
    let projects = (0..320)
        .map(|index| serde_json::json!({
            "project": format!("legacy-temper-{index:03}"),
            "repo_path": root.join("workspaces/engineer").join(format!("old-{index:03}/demo")).display().to_string(),
            "status": "stale",
            "updated_at_unix_secs": 1,
            "ownership": "temper",
            "estimated_bytes": 4096
        }))
        .chain([
            serde_json::json!({"project":"temper-v1-protected","repo_path":root.join("workspaces/engineer/stable/demo").display().to_string(),"status":"fresh","ownership":"temper","estimated_bytes":8192}),
            serde_json::json!({"project":"unrelated-project","repo_path":"/opt/unrelated/repo","status":"stale","estimated_bytes":2048}),
            serde_json::json!({"project":"ambiguous-project","status":"stale","estimated_bytes":1024}),
            serde_json::json!({"project":"active-project","repo_path":root.join("workspaces/engineer/active/demo").display().to_string(),"status":"stale","ownership":"temper","indexing_active":true,"estimated_bytes":4096}),
        ])
        .collect::<Vec<_>>();
    serde_json::json!({
        "cache_instance_id": "scenario-lifecycle-cache-v1",
        "cache_bytes": 1_327_104,
        "now_unix_secs": 2_000_000_000u64,
        "projects": projects
    })
}

fn validate_mcp_contract(mcp: &FakeMcpServer) -> Result<(), String> {
    let calls = logged_tool_calls(&mcp.log_path)?;
    let search = calls
        .iter()
        .filter(|call| call.name == "search_code")
        .collect::<Vec<_>>();
    if search.len() != 1 {
        return Err(format!(
            "expected one search_code MCP call, found {} in {}",
            search.len(),
            mcp.log_path.display()
        ));
    }
    let index = calls
        .iter()
        .filter(|call| call.name == "index_repository")
        .collect::<Vec<_>>();
    let indexed_project = index
        .first()
        .and_then(|call| call.arguments.get("name"))
        .and_then(JsonValue::as_str);
    if index.len() != 1
        || index[0].arguments.get("repo_path").is_none()
        || indexed_project.is_none()
    {
        return Err(format!(
            "stable index_repository was not exercised internally with repo_path and name: {calls:?}"
        ));
    }
    if search[0]
        .arguments
        .get("project")
        .and_then(JsonValue::as_str)
        != indexed_project
    {
        return Err(format!(
            "search_code did not reuse indexed stable project {:?}: {:?}",
            indexed_project, search[0].arguments
        ));
    }
    if calls.iter().any(|call| call.name == "list_projects") {
        return Err(format!(
            "normal startup called the global project inventory: {calls:?}"
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct McpToolCallEvidence {
    name: String,
    arguments: JsonValue,
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
            Some(McpToolCallEvidence { name, arguments })
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
            if messages_contain(view, "CODEBASE MEMORY") {
                observations_for_rule
                    .lock()
                    .expect("observations lock")
                    .prompt_guidance_seen = true;
            }
            if messages_contain(view, MEMORY_RESULT_NEEDLE) {
                observations_for_rule
                    .lock()
                    .expect("observations lock")
                    .memory_result_seen = true;
            }
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
        let (prompt_guidance_seen, memory_result_seen) = {
            let observations = self
                .observations
                .lock()
                .map_err(|_| "model observation mutex poisoned".to_string())?;
            (
                observations.prompt_guidance_seen,
                observations.memory_result_seen,
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
                "fake LLM did not receive the fake MCP search result\n{}",
                self.log_tail()
            ));
        }
        Ok(())
    }

    pub(super) fn log_tail(&self) -> String {
        let requests = self.fake.requests();
        let observations = self.observations.lock().expect("observations lock");
        let mut lines = vec![format!(
            "observations: prompt_guidance_seen={} memory_result_seen={}",
            observations.prompt_guidance_seen, observations.memory_result_seen
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
