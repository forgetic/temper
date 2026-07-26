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
    admin_forge, ci_diagnostics, completed_ci_jobs, issue_evidence, poll_until, pr_evidence,
    reject_labels, repository, require_labels, seed_intake,
};
use super::process::{
    TemperInitRequest, assert_init_workflow_yaml_matches, convergence_timeout, free_port,
    mint_site_admin_token, populate_repo, read_tail, run_temper_init, spawn_temper_standalone,
    tune_init_config, wait_for_standalone, write_snapshot,
};
use super::{
    ENGINEER, FakeLlmEvidence, FinalStateEvidence, LiveCodebaseMemoryEvidence, LiveLogPaths,
    LiveManifestEvidence, LiveManifestHarness,
};
use crate::forgejo_runtime::RunWorkspace;
use crate::forgejo_server::{ForgejoRunner, start_cached_bare_admin_server};

const ACTUAL_PROJECT: &str = "actual-demo-project";
const MEMORY_FILE: &str = "MEMORY_NOTES.md";
const MEMORY_RESULT_NEEDLE: &str = "FAKE_MCP_SEARCH_RESULT";
const ENGINEER_SUMMARY: &str = "Used codebase memory search result before writing MEMORY_NOTES.md.";

pub(super) fn run_tool_augmented_pull_request(
    harness: &LiveManifestHarness,
) -> Result<LiveManifestEvidence, String> {
    let started = Instant::now();
    harness.scenario.validate_workflow()?;

    let cached = start_cached_bare_admin_server(
        &harness.admin_user,
        &harness.admin_password,
        &harness.admin_email,
    )
    .map_err(|error| format!("cached bare-admin Forgejo starts: {error}"))?;
    let server = cached.server;
    let mut runner = ForgejoRunner::register(&server)
        .map_err(|error| format!("forgejo-runner registers: {error}"))?;
    if !runner.is_running() {
        return Err(format!(
            "forgejo-runner exited immediately\n--- runner log ---\n{}",
            runner.log_tail()
        ));
    }
    let admin_token = mint_site_admin_token(&server, &harness.admin_user)?;

    let fake = CodebaseMemoryFake::start(harness.scenario.jig_script_path())?;
    let scenario_run_id = super::scenario_run_id(&harness.scenario);
    let workspace = RunWorkspace::new(&harness.workspace_prefix);
    let bundle_dir = workspace.dir("bundle");
    let workspaces_dir = workspace.dir("workspaces");
    let logs = LiveLogPaths {
        workspace_root: workspace.path().to_path_buf(),
        init_log: workspace.join("logs/init.log"),
        repo_populate_log: workspace.join("logs/repo-populate.log"),
        standalone_log: workspace.join("logs/standalone.log"),
        fake_llm_log: workspace.join("logs/fake-llm.log"),
        ci_diagnostics_log: workspace.join("logs/ci-diagnostics.log"),
    };
    let fake_mcp = write_fake_mcp(workspace.path())?;

    let bind_port = free_port()?;
    run_temper_init(TemperInitRequest {
        temper: &harness.temper,
        server: &server,
        scenario: &harness.scenario,
        bundle_dir: &bundle_dir,
        workspaces_dir: &workspaces_dir,
        bind_port,
        fake_llm_url: &fake.base_url(),
        log: &logs.init_log,
        admin_user: &harness.admin_user,
        admin_password: &harness.admin_password,
        scenario_run_id: &scenario_run_id,
    })?;
    assert_init_workflow_yaml_matches(&bundle_dir.join("workflow.yaml"), &harness.scenario)?;
    tune_init_config(
        &bundle_dir.join("config.toml"),
        harness.scenario.poll_backstop.as_secs(),
        harness.scenario.mechanical_cadence.as_secs(),
    )?;
    tune_codebase_memory_config(&bundle_dir.join("config.toml"), &fake_mcp)?;

    populate_repo(
        server.base_url(),
        &admin_token,
        workspace.path(),
        &harness.scenario.repo,
        &logs.repo_populate_log,
    )?;

    let mut standalone = spawn_temper_standalone(
        &harness.temper,
        &bundle_dir,
        &logs.standalone_log,
        &harness.scenario.observability,
        &scenario_run_id,
    )?;
    wait_for_standalone(&mut standalone)?;

    let forge = admin_forge(server.base_url(), &admin_token, &harness.scenario.repo);
    let repository = super::process::engine_block_on(repository(&forge, &harness.scenario.repo))?;
    let issue = super::process::engine_block_on(seed_intake(
        &forge,
        &repository,
        &harness.scenario.intake,
    ))?;

    let timeout = convergence_timeout(harness.scenario.timeout);
    let convergence_start = Instant::now();
    let final_state = match drive_codebase_memory_convergence(
        &forge,
        &repository,
        issue,
        &harness.admin_user,
        &mut standalone,
        timeout,
    ) {
        Ok(final_state) => final_state,
        Err(error) => {
            retain_failure_logs(&logs, &fake_mcp, &fake, &forge, &repository);
            return Err(failure_report(
                timeout,
                &error,
                server.base_url(),
                &harness.scenario.repo.slug,
                runner.is_running(),
                &runner,
                &standalone,
                &logs,
                &fake_mcp,
                &fake,
                &forge,
                &repository,
            ));
        }
    };
    let convergence = convergence_start.elapsed();

    validate_mcp_contract(&fake_mcp.log_path)?;
    fake.validate_observations()?;
    if fake.engineer_requests() < 4 {
        return Err(format!(
            "fake LLM did not complete the codebase-memory engineer tool loop\n{}",
            fake.log_tail()
        ));
    }

    write_snapshot(&logs.fake_llm_log, &fake.log_tail());
    write_snapshot(
        &logs.ci_diagnostics_log,
        &ci_diagnostics(&forge, &repository),
    );

    standalone.kill();
    Ok(LiveManifestEvidence {
        _workspace: workspace,
        scenario_path: harness.scenario.scenario_path.clone(),
        manifest_path: harness.scenario.manifest_path.clone(),
        scenario_run_id,
        temper_log_format: harness.scenario.observability.log_format.clone(),
        rust_log: harness.scenario.observability.rust_log.clone(),
        temper_binary: harness.temper.binary().to_path_buf(),
        forge_url: server.base_url().to_string(),
        repo_slug: harness.scenario.repo.slug.clone(),
        repo_id: harness.scenario.repo.id.clone(),
        repo_default_branch: harness.scenario.repo.default_branch.clone(),
        forge_cache_hit: cached.cache_hit,
        runner_running: runner.is_running(),
        startup: started.elapsed().saturating_sub(convergence),
        convergence,
        total_elapsed: started.elapsed(),
        poll_backstop: harness.scenario.poll_backstop,
        fake_llm: FakeLlmEvidence {
            base_url: fake.base_url(),
            architect_requests: 0,
            engineer_requests: fake.engineer_requests(),
            tester_requests: 0,
            log_path: logs.fake_llm_log.clone(),
        },
        final_state,
        handoff: None,
        codebase_memory: Some(LiveCodebaseMemoryEvidence {
            produced_file: MEMORY_FILE.to_string(),
            expected_result: MEMORY_RESULT_NEEDLE.to_string(),
            fake_mcp_log: fake_mcp.log_path,
            mcp_search_calls: 1,
            safe_tools: vec![
                "codebase_memory_search_code".to_string(),
                "codebase_memory_list_projects".to_string(),
                "codebase_memory_index_status".to_string(),
            ],
            hidden_tools: vec![
                "codebase_memory_index_repository".to_string(),
                "codebase_memory_delete_project".to_string(),
            ],
        }),
        plan_feature: None,
        logs,
    })
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

    let jobs = completed_ci_jobs(forge, repository, &pr).await?;
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
    if !CiStatus::from_jobs(&jobs).is_passed() {
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

fn tune_codebase_memory_config(config_path: &Path, fake_mcp: &FakeMcpServer) -> Result<(), String> {
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
        TomlValue::String("required".to_string()),
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
            TomlValue::String(ACTUAL_PROJECT.to_string()),
        ]),
    );
    codebase.insert(
        "roles".to_string(),
        TomlValue::Array(vec![TomlValue::String("engineer".to_string())]),
    );
    codebase.insert(
        "index".to_string(),
        TomlValue::String("blocking".to_string()),
    );
    codebase.insert("startup_timeout_secs".to_string(), TomlValue::Integer(2));
    codebase.insert("index_timeout_secs".to_string(), TomlValue::Integer(3));
    tools.insert("codebase_memory".to_string(), TomlValue::Table(codebase));
    fs::write(
        config_path,
        toml::to_string_pretty(&doc).map_err(|error| format!("serialize tuned config: {error}"))?,
    )
    .map_err(|error| format!("write tuned config {}: {error}", config_path.display()))
}

struct FakeMcpServer {
    script_path: PathBuf,
    log_path: PathBuf,
}

fn write_fake_mcp(root: &Path) -> Result<FakeMcpServer, String> {
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
    })
}

fn validate_mcp_contract(log_path: &Path) -> Result<(), String> {
    let calls = logged_tool_calls(log_path)?;
    let search = calls
        .iter()
        .filter(|call| call.name == "search_code")
        .collect::<Vec<_>>();
    if search.len() != 1 {
        return Err(format!(
            "expected one search_code MCP call, found {} in {}",
            search.len(),
            log_path.display()
        ));
    }
    if search[0]
        .arguments
        .get("project")
        .and_then(JsonValue::as_str)
        != Some(ACTUAL_PROJECT)
    {
        return Err(format!(
            "search_code did not receive defaulted project {ACTUAL_PROJECT}: {:?}",
            search[0].arguments
        ));
    }
    let index = calls
        .iter()
        .filter(|call| call.name == "index_repository")
        .collect::<Vec<_>>();
    if index.len() != 1 || index[0].arguments.get("repo_path").is_none() {
        return Err(format!(
            "index_repository was not exercised internally with repo_path: {calls:?}"
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

fn retain_failure_logs(
    logs: &LiveLogPaths,
    fake_mcp: &FakeMcpServer,
    fake: &CodebaseMemoryFake,
    forge: &ForgejoForge,
    repository: &RepositoryId,
) {
    write_snapshot(&logs.fake_llm_log, &fake.log_tail());
    write_snapshot(&logs.ci_diagnostics_log, &ci_diagnostics(forge, repository));
    let _ = fs::copy(
        &fake_mcp.log_path,
        logs.workspace_root.join("fake-codebase-memory-mcp.jsonl"),
    );
}

#[allow(clippy::too_many_arguments)]
fn failure_report(
    timeout: Duration,
    error: &str,
    forge_url: &str,
    repo_slug: &str,
    runner_running: bool,
    runner: &ForgejoRunner,
    standalone: &super::process::ChildGuard,
    logs: &LiveLogPaths,
    fake_mcp: &FakeMcpServer,
    fake: &CodebaseMemoryFake,
    forge: &ForgejoForge,
    repository: &RepositoryId,
) -> String {
    format!(
        "live codebase-memory-agent did not converge within {timeout:?}: {error}\n\
         forge_url={forge_url} repo={repo_slug} runner_running={runner_running}\n\
         runner log tail:\n{}\n\
         --- init log ({}) ---\n{}\n\
         --- repo populate log ({}) ---\n{}\n\
         --- standalone daemon/worker/agent log ({}) ---\n{}\n\
         --- fake LLM request tail ({}) ---\n{}\n\
         --- fake MCP call log ({}) ---\n{}\n\
         --- CI diagnostics ({}) ---\n{}",
        runner.log_tail(),
        logs.init_log.display(),
        read_tail(&logs.init_log, 120),
        logs.repo_populate_log.display(),
        read_tail(&logs.repo_populate_log, 120),
        logs.standalone_log.display(),
        standalone.log_tail(),
        logs.fake_llm_log.display(),
        fake.log_tail(),
        fake_mcp.log_path.display(),
        read_tail(&fake_mcp.log_path, 80),
        logs.ci_diagnostics_log.display(),
        ci_diagnostics(forge, repository),
    )
}

struct CodebaseMemoryFake {
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
    fn start(script_path: &Path) -> Result<Self, String> {
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

    fn base_url(&self) -> String {
        self.fake.base_url()
    }

    fn engineer_requests(&self) -> usize {
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

    fn log_tail(&self) -> String {
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
