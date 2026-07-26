use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{
    BranchRef, CommitFile, CreateBranch, CreateIssue, CreatePullRequest, Forge, ForgeContent,
    ItemNumber, PullRequest, PullRequestQuery, PullRequestState, RepositoryId, UpdateIssue,
    UpdatePullRequest,
};
use temper_workflow::{
    ArtifactKindId, ArtifactRef, WorkflowMetadata, parse_metadata_block, render_metadata_block,
};
use toml::Value as TomlValue;

use super::convergence::{admin_forge, ci_diagnostics, repository};
use super::process::{
    TemperInitRequest, assert_init_workflow_yaml_matches, convergence_timeout, free_port,
    mint_site_admin_token, populate_repo, read_tail, run_temper_init, spawn_temper_standalone,
    tune_init_config, wait_for_standalone, write_snapshot,
};
use super::{
    FakeLlmEvidence, FinalStateEvidence, IssueEvidence, LiveLogPaths, LiveManifestEvidence,
    LiveManifestHarness, PullRequestEvidence,
};
use crate::forgejo_runtime::RunWorkspace;
use crate::forgejo_server::{ForgejoRunner, start_cached_bare_admin_server};

mod fake;

use fake::HandoffFake;

const REFRESH_FAKE_TIMEOUT: Duration = Duration::from_secs(60);
const ASSERT_POLL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveHandoffEvidence {
    pub create: LiveHandoffCaseEvidence,
    pub refresh: LiveHandoffCaseEvidence,
    pub stale_body: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveHandoffCaseEvidence {
    pub issue_number: u64,
    pub pr_number: u64,
    pub pr_state: String,
    pub labels: Vec<String>,
    pub head_branch: String,
    pub head_sha: Option<String>,
    pub title: String,
    pub body: String,
    pub body_prefix: String,
    pub correlation_key: String,
    pub source_artifact: String,
}

#[derive(Clone, Debug)]
struct HandoffFixture {
    create_issue: IssueFixture,
    refresh_issue: IssueFixture,
    create_title: String,
    create_body: String,
    refresh_title: String,
    refresh_body: String,
    stale_title: String,
    stale_body: String,
}

#[derive(Clone, Debug)]
struct IssueFixture {
    title: String,
    body: String,
    labels: Vec<String>,
}

pub(super) fn run_authored_pr_create_refresh(
    harness: &LiveManifestHarness,
) -> Result<LiveManifestEvidence, String> {
    let started = Instant::now();
    harness.scenario.validate_workflow()?;
    let fixture = HandoffFixture::load(
        &harness.scenario.scenario_path,
        &harness.scenario.resolved_manifest,
    )?;

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

    let fake = HandoffFake::start(harness.scenario.jig_script_path())?;
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
    let timeout = convergence_timeout(harness.scenario.timeout);
    let deadline = Instant::now() + timeout;

    let create_issue = super::process::engine_block_on(seed_issue(
        &forge,
        &repository,
        &fixture.create_issue,
        "create authored handoff",
        true,
    ))?;
    let stimuli = super::stimuli::execute_live_stimuli(
        &harness.scenario.execution.stimuli,
        super::stimuli::LiveStimulusResources {
            scenario: &harness.scenario,
            server: &server,
            runner: &mut runner,
            temper: &harness.temper,
            bundle_dir: &bundle_dir,
            logs: &logs,
            scenario_run_id: &scenario_run_id,
            standalone: &mut standalone,
            forge: &forge,
            repository: &repository,
            issue: create_issue,
        },
    )
    .map_err(|failure| {
        workspace.retain_on_drop();
        retain_failure_logs(&logs, &fake, &forge, &repository);
        format!(
            "declared live stimulus failed: {}\nstandalone log: {}\nCI diagnostics: {}",
            failure.diagnostic(),
            logs.standalone_log.display(),
            logs.ci_diagnostics_log.display()
        )
    })?;
    let create = match poll_handoff_case(
        deadline,
        &mut standalone,
        &forge,
        &repository,
        &harness.scenario.repo.slug,
        create_issue,
        &fixture.create_title,
        &fixture.create_body,
        None,
    ) {
        Ok(case) => case,
        Err(error) => {
            workspace.retain_on_drop();
            retain_failure_logs(&logs, &fake, &forge, &repository);
            return Err(failure_report(
                "create handoff did not converge",
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

    let refresh_issue = super::process::engine_block_on(seed_issue(
        &forge,
        &repository,
        &fixture.refresh_issue,
        "refresh existing handoff",
        false,
    ))?;
    let seeded = super::process::engine_block_on(seed_existing_pr(
        &forge,
        &repository,
        &harness.scenario.repo.default_branch,
        refresh_issue,
        &fixture,
    ))?;
    super::process::engine_block_on(mark_issue_ready(
        &forge,
        &harness.scenario.repo.slug,
        refresh_issue,
    ))?;
    fake.wait_for_refresh_started(REFRESH_FAKE_TIMEOUT)?;
    super::process::engine_block_on(mark_stale_pr_as_implementation(&forge, &seeded))?;
    fake.allow_refresh_continue();
    let refresh = match poll_handoff_case(
        deadline,
        &mut standalone,
        &forge,
        &repository,
        &harness.scenario.repo.slug,
        refresh_issue,
        &fixture.refresh_title,
        &fixture.refresh_body,
        Some(&fixture.stale_body),
    ) {
        Ok(case) => case,
        Err(error) => {
            workspace.retain_on_drop();
            retain_failure_logs(&logs, &fake, &forge, &repository);
            return Err(failure_report(
                "refresh handoff did not converge",
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
    if refresh.pr_number != seeded.number.get() {
        return Err(format!(
            "refresh opened PR #{} instead of updating existing PR #{}",
            refresh.pr_number, seeded.number
        ));
    }
    super::process::engine_block_on(assert_no_duplicate_for_branch(
        &forge,
        &repository,
        &branch_name(refresh_issue),
    ))?;

    write_snapshot(&logs.fake_llm_log, &fake.log_tail());
    write_snapshot(
        &logs.ci_diagnostics_log,
        &ci_diagnostics(&forge, &repository),
    );

    let convergence = started.elapsed();
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
        final_state: FinalStateEvidence {
            issue: IssueEvidence {
                number: create.issue_number,
                title: fixture.create_issue.title.clone(),
                state: "open".to_string(),
                labels: vec!["code".to_string(), "in-progress".to_string()],
            },
            pull_request: PullRequestEvidence {
                number: create.pr_number,
                title: create.title.clone(),
                state: create.pr_state.clone(),
                labels: create.labels.clone(),
                author: super::ENGINEER.to_string(),
                merged_by: None,
                head_branch: create.head_branch.clone(),
                head_sha: create.head_sha.clone(),
                merged_sha: None,
            },
            ci_jobs: Vec::new(),
        },
        handoff: Some(LiveHandoffEvidence {
            create,
            refresh,
            stale_body: fixture.stale_body,
        }),
        codebase_memory: None,
        plan_feature: None,
        stimuli,
        logs,
    })
}

fn retain_failure_logs(
    logs: &LiveLogPaths,
    fake: &HandoffFake,
    forge: &ForgejoForge,
    repository: &RepositoryId,
) {
    write_snapshot(&logs.fake_llm_log, &fake.log_tail());
    write_snapshot(&logs.ci_diagnostics_log, &ci_diagnostics(forge, repository));
}

#[allow(clippy::too_many_arguments)]
fn failure_report(
    phase: &str,
    timeout: Duration,
    error: &str,
    forge_url: &str,
    repo_slug: &str,
    runner_running: bool,
    runner: &ForgejoRunner,
    standalone: &super::process::ChildGuard,
    logs: &LiveLogPaths,
    fake: &HandoffFake,
    forge: &ForgejoForge,
    repository: &RepositoryId,
) -> String {
    format!(
        "live implementation-pr-handoff {phase} within {timeout:?}: {error}\n\
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

async fn seed_issue(
    forge: &impl Forge,
    repository: &RepositoryId,
    fixture: &IssueFixture,
    suffix: &str,
    ready: bool,
) -> Result<ItemNumber, String> {
    let mut labels = fixture.labels.clone();
    push_unique(&mut labels, "code");
    if ready {
        push_unique(&mut labels, "ready");
    }
    forge
        .create_issue(
            repository,
            CreateIssue {
                title: format!("{} ({suffix})", fixture.title),
                body: fixture.body.clone(),
                labels,
                assignees: Vec::new(),
            },
        )
        .await
        .map(|issue| issue.number)
        .map_err(|error| format!("create {suffix} source issue failed: {error}"))
}

async fn mark_issue_ready(
    forge: &impl Forge,
    repo_slug: &str,
    issue: ItemNumber,
) -> Result<(), String> {
    let issue_id =
        temper_forge_model::IssueId::new(format!("forgejo:{repo_slug}:issue:{}", issue.get()));
    forge
        .update_issue(
            &issue_id,
            UpdateIssue {
                add_labels: vec!["ready".to_string()],
                ..UpdateIssue::default()
            },
        )
        .await
        .map(|_| ())
        .map_err(|error| format!("mark refresh issue #{issue} ready failed: {error}"))
}

async fn mark_stale_pr_as_implementation(
    forge: &impl Forge,
    pull_request: &PullRequest,
) -> Result<(), String> {
    forge
        .update_pull_request(
            &pull_request.id,
            UpdatePullRequest {
                add_labels: vec!["implementation".to_string(), "in-progress".to_string()],
                ..UpdatePullRequest::default()
            },
        )
        .await
        .map(|_| ())
        .map_err(|error| {
            format!(
                "mark stale refresh PR #{} as implementation failed: {error}",
                pull_request.number
            )
        })
}

async fn seed_existing_pr(
    forge: &(impl Forge + ForgeContent),
    repository: &RepositoryId,
    default_branch: &str,
    issue: ItemNumber,
    fixture: &HandoffFixture,
) -> Result<PullRequest, String> {
    let branch = branch_name(issue);
    forge
        .create_branch(
            repository,
            CreateBranch {
                new_branch: branch.clone(),
                from_branch: default_branch.to_string(),
            },
        )
        .await
        .map_err(|error| format!("create refresh branch {branch}: {error}"))?;
    forge
        .commit_file(
            repository,
            CommitFile {
                path: "HANDOFF_REFRESH_STALE.md".to_string(),
                contents: fixture.stale_body.as_bytes().to_vec(),
                message: "seed stale implementation PR handoff".to_string(),
                branch: branch.clone(),
            },
        )
        .await
        .map_err(|error| format!("commit stale refresh fixture on {branch}: {error}"))?;

    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        parents: vec![ArtifactRef::same_repo(issue)],
        correlation_key: Some(correlation_key(issue)),
        ..WorkflowMetadata::default()
    };
    forge
        .create_pull_request(
            repository,
            CreatePullRequest {
                title: fixture.stale_title.clone(),
                body: format!(
                    "{}\n\n{}",
                    fixture.stale_body.trim(),
                    render_metadata_block(&metadata)
                ),
                source: BranchRef {
                    repository_id: repository.clone(),
                    branch,
                },
                target: BranchRef {
                    repository_id: repository.clone(),
                    branch: default_branch.to_string(),
                },
                labels: Vec::new(),
                assignees: Vec::new(),
            },
        )
        .await
        .map_err(|error| format!("create stale refresh implementation PR failed: {error}"))
}

async fn assert_no_duplicate_for_branch(
    forge: &impl Forge,
    repository: &RepositoryId,
    branch: &str,
) -> Result<(), String> {
    let pulls = forge
        .list_pull_requests(repository, PullRequestQuery::default())
        .await
        .map_err(|error| error.to_string())?;
    let matches = pulls
        .iter()
        .filter(|pull| pull.labels.iter().any(|label| label == "implementation"))
        .filter(|pull| pull.source.branch == branch)
        .count();
    if matches == 1 {
        Ok(())
    } else {
        Err(format!(
            "expected one implementation PR for branch `{branch}`, found {matches}"
        ))
    }
}

#[allow(clippy::too_many_arguments)]
fn poll_handoff_case(
    deadline: Instant,
    standalone: &mut super::process::ChildGuard,
    forge: &impl Forge,
    repository: &RepositoryId,
    repo_slug: &str,
    issue: ItemNumber,
    expected_title: &str,
    expected_body: &str,
    stale_body: Option<&str>,
) -> Result<LiveHandoffCaseEvidence, String> {
    loop {
        if let Some(status) = standalone.try_wait()? {
            return Err(format!("{} exited early with {status:?}", standalone.label));
        }
        match super::process::engine_block_on(verify_handoff_case(
            forge,
            repository,
            repo_slug,
            issue,
            expected_title,
            expected_body,
            stale_body,
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

async fn verify_handoff_case(
    forge: &impl Forge,
    repository: &RepositoryId,
    repo_slug: &str,
    issue: ItemNumber,
    expected_title: &str,
    expected_body: &str,
    stale_body: Option<&str>,
) -> Result<LiveHandoffCaseEvidence, String> {
    let correlation = correlation_key(issue);
    let pulls = forge
        .list_pull_requests(repository, PullRequestQuery::default())
        .await
        .map_err(|error| format!("list pull requests failed: {error}"))?;
    let pull = pulls
        .iter()
        .find(|pull| {
            pull.labels.iter().any(|label| label == "implementation")
                && parse_metadata_block(&pull.body)
                    .ok()
                    .flatten()
                    .and_then(|metadata| metadata.correlation_key)
                    .as_deref()
                    == Some(correlation.as_str())
        })
        .ok_or_else(|| format!("no implementation PR carried correlation `{correlation}`"))?;
    if pull.title != expected_title {
        return Err(format!(
            "PR #{} title mismatch: expected `{expected_title}`, got `{}`",
            pull.number, pull.title
        ));
    }
    let expected_prefix = expected_body.trim();
    if !pull.body.starts_with(expected_prefix) {
        return Err(format!(
            "PR #{} body did not start with authored report `{expected_prefix}`",
            pull.number
        ));
    }
    if let Some(stale_body) = stale_body.map(str::trim) {
        if !stale_body.is_empty() && pull.body.contains(stale_body) {
            return Err(format!(
                "PR #{} body still contained stale handoff text `{stale_body}`",
                pull.number
            ));
        }
    }
    let metadata = parse_metadata_block(&pull.body)
        .map_err(|error| format!("parse PR #{} metadata: {error}", pull.number))?
        .ok_or_else(|| format!("PR #{} had no workflow metadata block", pull.number))?;
    if metadata.kind != Some(ArtifactKindId::new("implementation_pr")) {
        return Err(format!(
            "PR #{} metadata kind mismatch: {:?}",
            pull.number, metadata.kind
        ));
    }
    if metadata.parents != vec![ArtifactRef::same_repo(issue)] {
        return Err(format!(
            "PR #{} metadata parents mismatch: {:?}",
            pull.number, metadata.parents
        ));
    }
    if metadata.correlation_key.as_deref() != Some(correlation.as_str()) {
        return Err(format!(
            "PR #{} metadata correlation mismatch: {:?}",
            pull.number, metadata.correlation_key
        ));
    }
    if !pull.labels.iter().any(|label| label == "landing") {
        return Err(format!(
            "PR #{} handoff labels not applied yet (labels {:?})",
            pull.number, pull.labels
        ));
    }
    if pull.labels.iter().any(|label| label == "in-progress") {
        return Err(format!(
            "PR #{} still has in-progress label after handoff (labels {:?})",
            pull.number, pull.labels
        ));
    }

    Ok(LiveHandoffCaseEvidence {
        issue_number: issue.get(),
        pr_number: pull.number.get(),
        pr_state: pr_state_evidence(pull.state).to_string(),
        labels: pull.labels.clone(),
        head_branch: pull.source.branch.clone(),
        head_sha: pull.head_sha.clone(),
        title: pull.title.clone(),
        body: pull.body.clone(),
        body_prefix: first_line(expected_prefix),
        correlation_key: correlation,
        source_artifact: format!("{repo_slug}#{}", issue.get()),
    })
}

impl HandoffFixture {
    fn load(scenario_path: &Path, manifest: &TomlValue) -> Result<Self, String> {
        let source = source_issue_fixture(scenario_path, manifest)?;
        let create_issue =
            issue_fixture(scenario_path, manifest, "create")?.unwrap_or_else(|| source.clone());
        let refresh_issue =
            issue_fixture(scenario_path, manifest, "refresh")?.unwrap_or_else(|| source.clone());
        let handoff = manifest
            .get("handoff")
            .and_then(TomlValue::as_table)
            .ok_or_else(|| {
                "implementation-pr-handoff manifest has no [handoff] section".to_string()
            })?;
        Ok(Self {
            create_issue,
            refresh_issue,
            create_title: required_string(handoff, "create_title")?,
            create_body: read_path_field(scenario_path, handoff, "create_body_path")?,
            refresh_title: required_string(handoff, "refresh_title")?,
            refresh_body: read_path_field(scenario_path, handoff, "refresh_body_path")?,
            stale_title: required_string(handoff, "stale_title")?,
            stale_body: required_string(handoff, "stale_body")?,
        })
    }
}

fn source_issue_fixture(
    scenario_path: &Path,
    manifest: &TomlValue,
) -> Result<IssueFixture, String> {
    if let Some(issue) = issue_fixture(scenario_path, manifest, "source")? {
        return Ok(issue);
    }
    if let Some(issue) = issue_fixture(scenario_path, manifest, "create")? {
        return Ok(issue);
    }
    Err("implementation-pr-handoff manifest has no source/create issue".to_string())
}

fn issue_fixture(
    scenario_path: &Path,
    manifest: &TomlValue,
    id: &str,
) -> Result<Option<IssueFixture>, String> {
    let Some(issue) = manifest
        .get("issues")
        .and_then(TomlValue::as_array)
        .and_then(|issues| {
            issues
                .iter()
                .filter_map(TomlValue::as_table)
                .find(|issue| issue.get("id").and_then(TomlValue::as_str) == Some(id))
        })
    else {
        return Ok(None);
    };
    let title = issue
        .get("title")
        .and_then(TomlValue::as_str)
        .ok_or_else(|| format!("issue `{id}` is missing `title`"))?
        .to_string();
    let body_ref = issue
        .get("body")
        .and_then(TomlValue::as_str)
        .ok_or_else(|| format!("issue `{id}` is missing `body`"))?;
    let body_path = scenario_path.join(body_ref);
    let body = fs::read_to_string(&body_path)
        .map_err(|error| format!("read issue `{id}` body {}: {error}", body_path.display()))?;
    let labels = issue
        .get("labels")
        .and_then(TomlValue::as_array)
        .map(|labels| {
            labels
                .iter()
                .map(|label| {
                    label
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("issue `{id}` labels must be strings"))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(Some(IssueFixture {
        title,
        body,
        labels,
    }))
}

fn required_string(table: &toml::Table, field: &str) -> Result<String, String> {
    table
        .get(field)
        .and_then(TomlValue::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("implementation-pr-handoff [handoff] is missing `{field}`"))
}

fn read_path_field(
    scenario_path: &Path,
    table: &toml::Table,
    field: &str,
) -> Result<String, String> {
    let path = required_string(table, field)?;
    let resolved = scenario_path.join(&path);
    fs::read_to_string(&resolved)
        .map_err(|error| format!("read {field} {}: {error}", resolved.display()))
}

fn branch_name(issue: ItemNumber) -> String {
    format!("agent/{}", correlation_key(issue))
}

fn correlation_key(issue: ItemNumber) -> String {
    format!("pr-for-code-{}", issue.get())
}

fn first_line(value: &str) -> String {
    value.lines().next().unwrap_or(value).trim().to_string()
}

fn pr_state_evidence(state: PullRequestState) -> &'static str {
    match state {
        PullRequestState::Open => "open",
        PullRequestState::Closed => "closed",
        PullRequestState::Merged => "merged",
    }
}

fn push_unique(labels: &mut Vec<String>, label: &str) {
    if !labels.iter().any(|candidate| candidate == label) {
        labels.push(label.to_string());
    }
}
