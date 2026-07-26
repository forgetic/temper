use std::process::Command;
use std::time::{Duration, Instant};

use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{CreateIssue, Forge, ItemNumber, RepositoryId};

#[path = "plan_feature/audit.rs"]
mod audit;
#[path = "plan_feature/fake.rs"]
mod fake;
#[path = "plan_feature/verify.rs"]
mod verify;

pub use audit::ValidationAuditEvidence;
use fake::PlanFeatureFake;
use verify::poll_plan_feature;

use super::convergence::{admin_forge, ci_diagnostics, repository};
use super::process::{
    TemperInitRequest, assert_init_workflow_yaml_matches, convergence_timeout, free_port,
    mint_site_admin_token, populate_repo, read_tail, run_temper_init, spawn_temper_standalone,
    tune_init_config, wait_for_standalone, write_snapshot,
};
use super::{
    CiJobEvidence, FakeLlmEvidence, FinalStateEvidence, IssueEvidence, LiveLogPaths,
    LiveManifestEvidence, LiveManifestHarness, PullRequestEvidence,
};
use crate::forgejo_runtime::RunWorkspace;
use crate::forgejo_server::{ForgejoRunner, start_cached_bare_admin_server};

const PLAN_TITLE: &str = "Plan plan-centric dogfood delivery";
const FIRST_CODE_TITLE: &str = "Implement plan foundation slice";
const SECOND_CODE_TITLE: &str = "Implement validation and landing slice";
const FOLLOWUP_CODE_TITLE: &str = "Implement validation follow-up regression";
const LANDING_TITLE: &str = "Land plan-centric dogfood feature branch";
const FOLLOWUP_VALIDATION_SUMMARY: &str =
    "Requested one implementation follow-up before aggregate landing.";
const VALIDATION_SUMMARY: &str =
    "Validated all feature-branch implementations and aggregate landing readiness.";
const ASSERT_POLL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LivePlanFeatureEvidence {
    pub feature_branch: String,
    pub feature_issue: IssueState,
    pub plan_issue: IssueState,
    pub first_code_issue: IssueState,
    pub second_code_issue: IssueState,
    pub followup_code_issue: IssueState,
    pub first_pr: PullRequestStateEvidence,
    pub second_pr: PullRequestStateEvidence,
    pub followup_pr: PullRequestStateEvidence,
    pub landing_pr: PullRequestStateEvidence,
    pub ci_jobs: Vec<PullRequestCiJobEvidence>,
    pub validation_audits: Vec<ValidationAuditEvidence>,
    pub prompt_guidance: Vec<RolePromptEvidence>,
    pub initial_main_sha: String,
    pub main_sha_before_landing: String,
    pub final_main_sha: String,
    pub observed_second_blocked: bool,
    pub observed_second_unblocked: bool,
    pub observed_landing_open_with_parents_open: bool,
    pub validation_waited_for_implementations: bool,
    pub ci_green_before_merge: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueState {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub labels: Vec<String>,
    pub target_branch: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PullRequestStateEvidence {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub labels: Vec<String>,
    pub source_branch: String,
    pub target_branch: String,
    pub head_sha: Option<String>,
    pub base_sha: Option<String>,
    pub merged_sha: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PullRequestCiJobEvidence {
    pub pull_request_number: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RolePromptEvidence {
    pub role: String,
    pub request_count: usize,
    pub role_guidance_excerpt: String,
    pub prompt_guidance_excerpt: String,
    pub tool_guidance_excerpt: String,
    pub constraint_excerpts: Vec<String>,
}

pub(super) fn run_feature_branch_aggregate_landing(
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

    let fake = PlanFeatureFake::start(harness.scenario.jig_script_path())?;
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

    populate_repo(
        server.base_url(),
        &admin_token,
        workspace.path(),
        &harness.scenario.repo,
        &logs.repo_populate_log,
    )?;
    let initial_main_sha = local_checkout_head(
        &workspace
            .path()
            .join("repo-seed")
            .join(&harness.scenario.repo.name),
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
    let feature_issue = super::process::engine_block_on(seed_feature_issue(
        &forge,
        &repository,
        &harness.scenario.intake,
    ))?;

    let timeout = convergence_timeout(harness.scenario.timeout);
    let convergence_start = Instant::now();
    let deadline = Instant::now() + timeout;
    let mut plan_feature = match poll_plan_feature(
        deadline,
        &mut standalone,
        &forge,
        &repository,
        feature_issue,
        &harness.scenario.repo.default_branch,
        &initial_main_sha,
        server.base_url(),
        &admin_token,
        &harness.scenario.repo.owner,
        &harness.scenario.repo.name,
    ) {
        Ok(evidence) => evidence,
        Err(error) => {
            retain_failure_logs(&logs, &fake, &forge, &repository);
            return Err(failure_report(
                timeout,
                &error,
                server.base_url(),
                &harness.scenario.repo.slug,
                runner.is_running(),
                &runner,
                &standalone,
                &logs,
                &fake,
                &forge,
                &repository,
            ));
        }
    };

    for (role, actual, minimum) in [
        ("architect", fake.architect_requests(), 4),
        ("engineer", fake.engineer_requests(), 9),
        ("tester", fake.tester_requests(), 4),
    ] {
        if actual < minimum {
            return Err(format!(
                "fake LLM served only {actual} {role} requests; expected at least {minimum}\n{}",
                fake.log_tail()
            ));
        }
    }
    plan_feature.prompt_guidance = fake.prompt_guidance_evidence()?;
    plan_feature.final_main_sha = remote_branch_head(
        server.base_url(),
        &admin_token,
        &harness.scenario.repo.owner,
        &harness.scenario.repo.name,
        &harness.scenario.repo.default_branch,
    )?;
    if plan_feature.final_main_sha
        != plan_feature
            .landing_pr
            .merged_sha
            .clone()
            .unwrap_or_default()
    {
        return Err(format!(
            "main did not end at the aggregate landing merge: main={} landing={:?}",
            plan_feature.final_main_sha, plan_feature.landing_pr.merged_sha
        ));
    }

    write_snapshot(&logs.fake_llm_log, &fake.log_tail());
    write_snapshot(
        &logs.ci_diagnostics_log,
        &ci_diagnostics(&forge, &repository),
    );

    let convergence = convergence_start.elapsed();
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
            architect_requests: fake.architect_requests(),
            engineer_requests: fake.engineer_requests(),
            tester_requests: fake.tester_requests(),
            log_path: logs.fake_llm_log.clone(),
        },
        final_state: FinalStateEvidence {
            issue: IssueEvidence {
                number: plan_feature.feature_issue.number,
                title: plan_feature.feature_issue.title.clone(),
                state: plan_feature.feature_issue.state.clone(),
                labels: plan_feature.feature_issue.labels.clone(),
            },
            pull_request: PullRequestEvidence {
                number: plan_feature.landing_pr.number,
                title: plan_feature.landing_pr.title.clone(),
                state: plan_feature.landing_pr.state.clone(),
                labels: plan_feature.landing_pr.labels.clone(),
                author: super::ENGINEER.to_string(),
                merged_by: None,
                head_branch: plan_feature.landing_pr.source_branch.clone(),
                head_sha: plan_feature.landing_pr.head_sha.clone(),
                merged_sha: plan_feature.landing_pr.merged_sha.clone(),
            },
            ci_jobs: plan_feature
                .ci_jobs
                .iter()
                .map(|job| CiJobEvidence {
                    name: job.name.clone(),
                    status: job.status.clone(),
                    conclusion: job.conclusion.clone(),
                    url: job.url.clone(),
                })
                .collect(),
        },
        handoff: None,
        codebase_memory: None,
        plan_feature: Some(plan_feature),
        logs,
    })
}

async fn seed_feature_issue(
    forge: &impl Forge,
    repository: &RepositoryId,
    issue: &super::IntakeFixture,
) -> Result<ItemNumber, String> {
    forge
        .create_issue(
            repository,
            CreateIssue {
                title: issue.title.clone(),
                body: issue.body.clone(),
                labels: issue.labels.clone(),
                assignees: Vec::new(),
            },
        )
        .await
        .map(|issue| issue.number)
        .map_err(|error| format!("create feature issue failed: {error}"))
}

fn local_checkout_head(checkout: &std::path::Path) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(checkout)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|error| format!("read seeded repository head: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "read seeded repository head failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .filter(|sha| !sha.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "seeded repository did not expose a HEAD SHA".to_string())
}

pub(super) fn remote_branch_head(
    base_url: &str,
    token: &str,
    owner: &str,
    repo: &str,
    branch: &str,
) -> Result<String, String> {
    super::process::engine_block_on(async {
        let response = temper_engine_io::http::JsonClient::new()
            .send(
                "GET",
                format!("{base_url}/api/v1/repos/{owner}/{repo}/branches/{branch}"),
                Some(token),
                None,
            )
            .await
            .map_err(|error| format!("query remote branch {branch}: {error}"))?;
        if !(200..300).contains(&response.status) {
            return Err(format!(
                "query remote branch {branch} returned HTTP {}: {}",
                response.status,
                String::from_utf8_lossy(&response.body)
            ));
        }
        let body: serde_json::Value = serde_json::from_slice(&response.body)
            .map_err(|error| format!("parse remote branch {branch}: {error}"))?;
        body.pointer("/commit/id")
            .or_else(|| body.pointer("/commit/sha"))
            .and_then(serde_json::Value::as_str)
            .filter(|sha| !sha.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("remote branch {branch} did not expose a commit id: {body}"))
    })
}

fn retain_failure_logs(
    logs: &LiveLogPaths,
    fake: &PlanFeatureFake,
    forge: &ForgejoForge,
    repository: &RepositoryId,
) {
    write_snapshot(&logs.fake_llm_log, &fake.log_tail());
    write_snapshot(&logs.ci_diagnostics_log, &ci_diagnostics(forge, repository));
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
    fake: &PlanFeatureFake,
    forge: &ForgejoForge,
    repository: &RepositoryId,
) -> String {
    format!(
        "live plan-centric feature branch scenario within {timeout:?}: {error}\n\
         forge_url={forge_url} repo={repo_slug} runner_running={runner_running}\n\
         runner log tail:\n{}\n\
         --- init log ({}) ---\n{}\n\
         --- repo populate log ({}) ---\n{}\n\
         --- standalone daemon/worker/agent log ({}) ---\n{}\n\
         --- fake LLM request tail ({}) ---\n{}\n\
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
        logs.ci_diagnostics_log.display(),
        ci_diagnostics(forge, repository),
    )
}
