use std::fs;
use std::time::{Duration, Instant};
use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{
    CreateIssue, Forge, Issue, IssueQuery, ItemNumber, PullRequest, PullRequestQuery,
    PullRequestState, RepositoryId,
};
#[path = "plan_feature/audit.rs"]
mod audit;
#[path = "plan_feature/fake.rs"]
mod fake;

pub use audit::ValidationAuditEvidence;
use audit::validation_audit_evidence;
use fake::PlanFeatureFake;

use super::convergence::{
    admin_forge, ci_diagnostics, ci_job_evidence, completed_ci_jobs, repository,
};
use super::process::{
    TemperInitRequest, assert_init_workflow_yaml_matches, convergence_timeout, free_port,
    mint_site_admin_token, populate_repo, read_tail, run_temper_init, spawn_temper_standalone,
    tune_init_config, wait_for_standalone, write_snapshot,
};
use super::{
    CiJobEvidence, FakeLlmEvidence, FinalStateEvidence, IssueEvidence, LiveBasicDeliveryEvidence,
    LiveBasicDeliveryHarness, LiveLogPaths, PullRequestEvidence,
};
use crate::forgejo_runtime::RunWorkspace;
use crate::forgejo_server::{ForgejoRunner, start_cached_bare_admin_server};

const FEATURE_BRANCH: &str = "feature/plan-centric-dogfood";
const FEATURE_TITLE: &str = "Ship plan-centric dogfood feature branch delivery";
const PLAN_TITLE: &str = "Plan plan-centric dogfood delivery";
const FIRST_CODE_TITLE: &str = "Implement plan foundation slice";
const SECOND_CODE_TITLE: &str = "Implement validation and landing slice";
const LANDING_TITLE: &str = "Land plan-centric dogfood feature branch";
const VALIDATION_SUMMARY: &str =
    "Validated the sequential feature-branch implementation and aggregate landing readiness.";
const ASSERT_POLL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LivePlanFeatureEvidence {
    pub feature_branch: String,
    pub feature_issue: IssueState,
    pub plan_issue: IssueState,
    pub first_code_issue: IssueState,
    pub second_code_issue: IssueState,
    pub first_pr: PullRequestStateEvidence,
    pub second_pr: PullRequestStateEvidence,
    pub landing_pr: PullRequestStateEvidence,
    pub ci_jobs: Vec<PullRequestCiJobEvidence>,
    pub validation_audit: ValidationAuditEvidence,
    pub observed_second_blocked: bool,
    pub observed_second_unblocked: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueState {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub labels: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PullRequestStateEvidence {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub labels: Vec<String>,
    pub source_branch: String,
    pub target_branch: String,
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

#[derive(Default)]
struct Observations {
    second_blocked: bool,
    second_unblocked_after_first_closed: bool,
}

pub(super) fn run_live_plan_feature_branch(
    harness: &LiveBasicDeliveryHarness,
) -> Result<LiveBasicDeliveryEvidence, String> {
    let started = Instant::now();
    harness.scenario.assert_workflow_matches_reference()?;

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

    let fake = PlanFeatureFake::start();
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
        &harness
            .scenario
            .scenario_path
            .join("config/feature-issue.md"),
    ))?;

    let timeout = convergence_timeout(harness.scenario.timeout);
    let convergence_start = Instant::now();
    let deadline = Instant::now() + timeout;
    let plan_feature = match poll_plan_feature(
        deadline,
        &mut standalone,
        &forge,
        &repository,
        feature_issue,
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

    if fake.architect_requests() < 2 {
        return Err(format!(
            "fake LLM never served both architect loops\n{}",
            fake.log_tail()
        ));
    }
    if fake.engineer_requests() < 4 {
        return Err(format!(
            "fake LLM never served both engineer tool loops\n{}",
            fake.log_tail()
        ));
    }
    if fake.tester_requests() < 2 {
        return Err(format!(
            "fake LLM never served the tester tool loop\n{}",
            fake.log_tail()
        ));
    }

    write_snapshot(&logs.fake_llm_log, &fake.log_tail());
    write_snapshot(
        &logs.ci_diagnostics_log,
        &ci_diagnostics(&forge, &repository),
    );

    let convergence = convergence_start.elapsed();
    standalone.kill();
    Ok(LiveBasicDeliveryEvidence {
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
                head_sha: None,
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
    body_path: &std::path::Path,
) -> Result<ItemNumber, String> {
    let body = fs::read_to_string(body_path)
        .map_err(|error| format!("read feature issue body {}: {error}", body_path.display()))?;
    forge
        .create_issue(
            repository,
            CreateIssue {
                title: FEATURE_TITLE.to_string(),
                body,
                labels: vec!["feature".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .map(|issue| issue.number)
        .map_err(|error| format!("create feature issue failed: {error}"))
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

fn poll_plan_feature(
    deadline: Instant,
    standalone: &mut super::process::ChildGuard,
    forge: &ForgejoForge,
    repository: &RepositoryId,
    feature_issue: ItemNumber,
) -> Result<LivePlanFeatureEvidence, String> {
    let mut observations = Observations::default();
    loop {
        if let Some(status) = standalone.try_wait()? {
            return Err(format!("{} exited early with {status:?}", standalone.label));
        }
        match super::process::engine_block_on(verify_plan_feature(
            forge,
            repository,
            feature_issue,
            &mut observations,
        )) {
            Ok(value) => return Ok(value),
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                std::thread::sleep(ASSERT_POLL);
            }
            Err(error) => return Err(error),
        }
    }
}

async fn verify_plan_feature(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    feature_issue: ItemNumber,
    observations: &mut Observations,
) -> Result<LivePlanFeatureEvidence, String> {
    let issues = forge
        .list_issues(repository, IssueQuery::default())
        .await
        .map_err(|error| format!("list issues: {error}"))?;
    let pulls = forge
        .list_pull_requests(repository, PullRequestQuery::default())
        .await
        .map_err(|error| format!("list pull requests: {error}"))?;

    let feature = issue_by_number(&issues, feature_issue)?;
    let plan = issue_by_label_and_title(&issues, "plan", PLAN_TITLE)?;
    let first = issue_by_label_and_title(&issues, "code", FIRST_CODE_TITLE)
        .map_err(|error| format!("{error}\n{}", describe_state(&issues, &pulls)))?;
    let second = issue_by_label_and_title(&issues, "code", SECOND_CODE_TITLE)
        .map_err(|error| format!("{error}\n{}", describe_state(&issues, &pulls)))?;

    if has_label(second, "blocked") {
        observations.second_blocked = true;
    }
    if issue_closed(first) && !has_label(second, "blocked") {
        observations.second_unblocked_after_first_closed = true;
    }

    let implementation_prs = pulls
        .iter()
        .filter(|pull| has_pr_label(pull, "implementation"))
        .collect::<Vec<_>>();
    let first_pr = pr_by_title(&implementation_prs, FIRST_CODE_TITLE)?;
    let second_pr = pr_by_title(&implementation_prs, SECOND_CODE_TITLE)?;
    let landing_pr = pulls
        .iter()
        .find(|pull| has_pr_label(pull, "feature-landing"))
        .ok_or_else(|| "feature landing PR was not created yet".to_string())?;

    require_pr_target(first_pr, FEATURE_BRANCH)?;
    require_pr_target(second_pr, FEATURE_BRANCH)?;
    require_pr_branch(landing_pr, FEATURE_BRANCH, "main")?;

    if !observations.second_blocked {
        return Err("downstream code issue was not observed with blocked label".to_string());
    }
    if !observations.second_unblocked_after_first_closed {
        return Err(
            "downstream code issue was not observed unblocked after the first code issue closed"
                .to_string(),
        );
    }
    if !issue_closed(first) || !issue_closed(second) {
        return Err("code issues are not both closed yet".to_string());
    }
    if !matches!(first_pr.state, PullRequestState::Merged)
        || !matches!(second_pr.state, PullRequestState::Merged)
    {
        return Err("implementation PRs are not both merged yet".to_string());
    }
    if !matches!(landing_pr.state, PullRequestState::Merged) {
        return Err("feature landing PR is not merged yet".to_string());
    }
    let ci_jobs =
        merged_pr_ci_evidence(forge, repository, &[first_pr, second_pr, landing_pr]).await?;
    if !issue_closed(feature) || !issue_closed(plan) {
        return Err("feature and plan issues are not both closed yet".to_string());
    }
    let validation_audit = validation_audit_evidence(forge, plan, VALIDATION_SUMMARY).await?;

    Ok(LivePlanFeatureEvidence {
        feature_branch: FEATURE_BRANCH.to_string(),
        feature_issue: issue_state(feature),
        plan_issue: issue_state(plan),
        first_code_issue: issue_state(first),
        second_code_issue: issue_state(second),
        first_pr: pr_state(first_pr),
        second_pr: pr_state(second_pr),
        landing_pr: pr_state(landing_pr),
        ci_jobs,
        validation_audit,
        observed_second_blocked: observations.second_blocked,
        observed_second_unblocked: observations.second_unblocked_after_first_closed,
    })
}

async fn merged_pr_ci_evidence(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    pulls: &[&PullRequest],
) -> Result<Vec<PullRequestCiJobEvidence>, String> {
    let mut evidence = Vec::new();
    for pull in pulls {
        let jobs = completed_ci_jobs(forge, repository, pull).await?;
        if jobs.is_empty() {
            return Err(format!("no completed CI jobs for PR #{}", pull.number));
        }
        if jobs.last().and_then(|job| job.conclusion)
            != Some(temper_forge_model::CiJobConclusion::Success)
        {
            return Err(format!(
                "latest completed CI job for PR #{} was not successful: {:?}",
                pull.number,
                jobs.last().and_then(|job| job.conclusion)
            ));
        }
        evidence.extend(jobs.iter().map(|job| {
            let job = ci_job_evidence(job);
            PullRequestCiJobEvidence {
                pull_request_number: pull.number.get(),
                name: job.name,
                status: job.status,
                conclusion: job.conclusion,
                url: job.url,
            }
        }));
    }
    Ok(evidence)
}

fn issue_by_number(issues: &[Issue], number: ItemNumber) -> Result<&Issue, String> {
    issues
        .iter()
        .find(|issue| issue.number == number)
        .ok_or_else(|| format!("issue #{number} not found"))
}

fn issue_by_label_and_title<'a>(
    issues: &'a [Issue],
    label: &str,
    title: &str,
) -> Result<&'a Issue, String> {
    issues
        .iter()
        .find(|issue| has_label(issue, label) && issue.title == title)
        .ok_or_else(|| format!("issue `{title}` with label `{label}` not found yet"))
}

fn pr_by_title<'a>(pulls: &'a [&PullRequest], title: &str) -> Result<&'a PullRequest, String> {
    pulls
        .iter()
        .copied()
        .find(|pull| pull.title == title)
        .ok_or_else(|| format!("implementation PR `{title}` not found yet"))
}

fn require_pr_branch(
    pull: &PullRequest,
    source_branch: &str,
    target_branch: &str,
) -> Result<(), String> {
    if pull.source.branch != source_branch || pull.target.branch != target_branch {
        return Err(format!(
            "PR #{} branch mismatch: expected {source_branch}->{target_branch}, got {}->{}",
            pull.number, pull.source.branch, pull.target.branch
        ));
    }
    Ok(())
}

fn require_pr_target(pull: &PullRequest, target_branch: &str) -> Result<(), String> {
    if pull.target.branch != target_branch {
        return Err(format!(
            "PR #{} target mismatch: expected {target_branch}, got {}->{}",
            pull.number, pull.source.branch, pull.target.branch
        ));
    }
    Ok(())
}

fn describe_state(issues: &[Issue], pulls: &[PullRequest]) -> String {
    let issues = issues
        .iter()
        .map(|issue| {
            format!(
                "issue #{} {:?} title={:?} labels={:?} deps={:?}",
                issue.number, issue.state, issue.title, issue.labels, issue.dependencies
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let pulls = pulls
        .iter()
        .map(|pull| {
            format!(
                "pr #{} {:?} title={:?} labels={:?} branch {}->{} deps={:?}",
                pull.number,
                pull.state,
                pull.title,
                pull.labels,
                pull.source.branch,
                pull.target.branch,
                pull.dependencies
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("observed issues:\n{issues}\nobserved pull requests:\n{pulls}")
}

fn has_label(issue: &Issue, label: &str) -> bool {
    issue.labels.iter().any(|candidate| candidate == label)
}

fn issue_closed(issue: &Issue) -> bool {
    matches!(issue.state, temper_forge_model::IssueState::Closed)
}

fn has_pr_label(pull: &PullRequest, label: &str) -> bool {
    pull.labels.iter().any(|candidate| candidate == label)
}

fn issue_state(issue: &Issue) -> IssueState {
    IssueState {
        number: issue.number.get(),
        title: issue.title.clone(),
        state: if issue_closed(issue) {
            "closed"
        } else {
            "open"
        }
        .to_string(),
        labels: issue.labels.clone(),
    }
}

fn pr_state(pull: &PullRequest) -> PullRequestStateEvidence {
    PullRequestStateEvidence {
        number: pull.number.get(),
        title: pull.title.clone(),
        state: match pull.state {
            PullRequestState::Open => "open",
            PullRequestState::Closed => "closed",
            PullRequestState::Merged => "merged",
        }
        .to_string(),
        labels: pull.labels.clone(),
        source_branch: pull.source.branch.clone(),
        target_branch: pull.target.branch.clone(),
        merged_sha: pull.merge.as_ref().map(|merge| merge.commit_sha.clone()),
    }
}
