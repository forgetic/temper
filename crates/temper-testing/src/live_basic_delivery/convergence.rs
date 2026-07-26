use std::time::{Duration, Instant};

use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_forge_model::{
    CiJob, CiJobConclusion, CiJobQuery, CiJobStatus, CreateIssue, Issue, IssueState, ItemNumber,
    PullRequest, PullRequestQuery, PullRequestState, RepositoryId, RepositoryPath, UserId,
};
use temper_workflow::{CiStatus, parse_metadata_block};

use super::fake_llm::{architect_body, engineer_summary};
use super::process::{ChildGuard, engine_block_on};
use super::{
    CiJobEvidence, ENGINEER, FinalStateEvidence, IntakeFixture, IssueEvidence, PullRequestEvidence,
    RepoFixture,
};

const ASSERT_POLL: Duration = Duration::from_secs(1);

pub(super) fn admin_forge(base_url: &str, admin_token: &str, repo: &RepoFixture) -> ForgejoForge {
    ForgejoForge::new(
        ForgejoConfig::new(base_url, admin_token).with_default_repo(&repo.owner, &repo.name),
    )
}

pub(super) async fn repository(
    forge: &ForgejoForge,
    repo: &RepoFixture,
) -> Result<RepositoryId, String> {
    forge
        .get_repository_by_path(&RepositoryPath::new(&repo.owner, &repo.name))
        .await
        .map_err(|error| format!("repository lookup failed: {error}"))?
        .map(|repo| repo.id)
        .ok_or_else(|| format!("repository {} not found", repo.slug))
}

pub(super) async fn seed_intake(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    intake: &IntakeFixture,
) -> Result<ItemNumber, String> {
    let issue = forge
        .create_issue(
            repository,
            CreateIssue {
                title: intake.title.clone(),
                body: intake.body.clone(),
                labels: intake.labels.clone(),
                assignees: Vec::new(),
            },
        )
        .await
        .map_err(|error| format!("create intake issue failed: {error}"))?;
    Ok(issue.number)
}

pub(super) fn drive_full_basic_delivery(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
    admin_user: &str,
    standalone: &mut ChildGuard,
    timeout: Duration,
) -> Result<FinalStateEvidence, String> {
    let deadline = Instant::now() + timeout;

    poll_until(deadline, standalone, || {
        engine_block_on(assert_basic_delivery_reached(
            forge,
            repository,
            issue,
            admin_user,
            BasicDeliveryPhase::Untriaged,
        ))
    })?;
    poll_until(deadline, standalone, || {
        engine_block_on(assert_basic_delivery_reached(
            forge,
            repository,
            issue,
            admin_user,
            BasicDeliveryPhase::TriagedReady,
        ))
    })?;
    poll_until(deadline, standalone, || {
        engine_block_on(assert_basic_delivery_reached(
            forge,
            repository,
            issue,
            admin_user,
            BasicDeliveryPhase::ImplementationPrOpen,
        ))
    })?;
    poll_until(deadline, standalone, || {
        engine_block_on(assert_converged(forge, repository, issue, admin_user))
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum BasicDeliveryPhase {
    Untriaged,
    TriagedReady,
    ImplementationPrOpen,
}

impl BasicDeliveryPhase {
    fn description(self) -> &'static str {
        match self {
            Self::Untriaged => "untriaged issue",
            Self::TriagedReady => "triaged-ready issue",
            Self::ImplementationPrOpen => "open implementation PR",
        }
    }
}

async fn assert_basic_delivery_reached(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
    admin_user: &str,
    minimum: BasicDeliveryPhase,
) -> Result<(), String> {
    let mut errors = Vec::new();

    if minimum <= BasicDeliveryPhase::Untriaged {
        match assert_issue_has_label(forge, repository, issue, "untriaged").await {
            Ok(()) => return Ok(()),
            Err(error) => errors.push((BasicDeliveryPhase::Untriaged.description(), error)),
        }
    }
    if minimum <= BasicDeliveryPhase::TriagedReady {
        match assert_issue_triaged_ready(forge, repository, issue).await {
            Ok(()) => return Ok(()),
            Err(error) => errors.push((BasicDeliveryPhase::TriagedReady.description(), error)),
        }
    }
    if minimum <= BasicDeliveryPhase::ImplementationPrOpen {
        match assert_pr_open_with_landing(forge, repository, issue).await {
            Ok(()) => return Ok(()),
            Err(error) => errors.push((
                BasicDeliveryPhase::ImplementationPrOpen.description(),
                error,
            )),
        }
    }

    match assert_converged(forge, repository, issue, admin_user).await {
        Ok(_) => Ok(()),
        Err(error) => {
            errors.push(("final convergence", error));
            Err(format_phase_wait_errors(minimum, &errors))
        }
    }
}

fn format_phase_wait_errors(
    minimum: BasicDeliveryPhase,
    errors: &[(&'static str, String)],
) -> String {
    let details = errors
        .iter()
        .map(|(phase, error)| format!("{phase}: {error}"))
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "workflow has not reached {} or any later valid phase yet ({details})",
        minimum.description()
    )
}

async fn assert_issue_has_label(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
    label: &str,
) -> Result<(), String> {
    let issue = forge
        .get_issue_by_number(repository, issue)
        .await
        .map_err(|error| format!("issue lookup failed: {error}"))?
        .ok_or("source issue disappeared")?;
    if issue.labels.iter().any(|have| have == label) {
        Ok(())
    } else {
        Err(format!(
            "issue #{} has not been mechanically marked `{label}` yet (labels {:?})",
            issue.number, issue.labels
        ))
    }
}

async fn assert_issue_triaged_ready(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
) -> Result<(), String> {
    let issue = forge
        .get_issue_by_number(repository, issue)
        .await
        .map_err(|error| format!("issue lookup failed: {error}"))?
        .ok_or("source issue disappeared")?;
    require_labels(&issue.labels, &["code", "ready"])?;
    reject_labels(&issue.labels, &["untriaged", "in-progress"])?;
    let expected_body = architect_body();
    if issue.body.trim() != expected_body.trim() {
        return Err(format!(
            "architect-authored body not applied yet\nexpected:\n{expected_body}\nactual:\n{}",
            issue.body
        ));
    }
    if !issue
        .assignees
        .iter()
        .any(|assignee| assignee == &UserId::new("architect"))
    {
        return Err(format!(
            "triaged issue is not assigned to architect yet (assignees {:?})",
            issue.assignees
        ));
    }
    Ok(())
}

async fn assert_pr_open_with_landing(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issue: ItemNumber,
) -> Result<(), String> {
    let pr = implementation_pr(forge, repository, issue).await?;
    verify_engineer_pr(&pr, issue)?;
    if pr.state != PullRequestState::Open {
        return Err(format!(
            "implementation PR #{} is not in the open landing state (state {:?})",
            pr.number, pr.state
        ));
    }
    require_labels(&pr.labels, &["implementation", "landing"])?;
    let expected_summary = engineer_summary();
    if !pr.body.contains(expected_summary) {
        return Err(format!(
            "implementation PR body does not contain engineer summary {:?}:\n{}",
            expected_summary, pr.body
        ));
    }
    Ok(())
}

async fn assert_converged(
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
    if merge.merged_by == UserId::new(ENGINEER) {
        return Err("PR was merged by the engineer, not mechanical automation".to_string());
    }
    let expected_automation = [UserId::new(admin_user), UserId::new("bot")];
    if !expected_automation
        .iter()
        .any(|user| user == &merge.merged_by)
    {
        return Err(format!(
            "PR was merged by {:?}, expected an automation identity ({:?})",
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
    let expected_body = architect_body().trim();
    if !issue.body.contains(expected_body) {
        return Err(format!(
            "terminal source issue no longer carries the architect-authored spec\nexpected to find:\n{expected_body}\nactual:\n{}",
            issue.body
        ));
    }

    Ok(FinalStateEvidence {
        issue: issue_evidence(&issue),
        pull_request: pr_evidence(&pr),
        ci_jobs: jobs.iter().map(ci_job_evidence).collect(),
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

pub(super) async fn completed_ci_jobs(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    pr: &PullRequest,
) -> Result<Vec<CiJob>, String> {
    let mut jobs = forge
        .list_ci_jobs(
            repository,
            CiJobQuery {
                pull_request_id: Some(pr.id.clone()),
                status: Some(CiJobStatus::Completed),
                ..CiJobQuery::default()
            },
        )
        .await
        .map_err(|error| format!("list_ci_jobs failed: {error}"))?;
    jobs.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then(left.id.cmp(&right.id))
    });
    Ok(jobs)
}

pub(super) fn ci_diagnostics(forge: &ForgejoForge, repository: &RepositoryId) -> String {
    engine_block_on(async {
        let mut out = String::new();
        match forge
            .list_pull_requests(repository, PullRequestQuery::default())
            .await
        {
            Ok(prs) => {
                for pr in &prs {
                    out.push_str(&format!(
                        "PR #{} head={} labels={:?} state={:?} merge={:?}\n",
                        pr.number, pr.source.branch, pr.labels, pr.state, pr.merge
                    ));
                    match forge
                        .list_ci_jobs(
                            repository,
                            CiJobQuery {
                                pull_request_id: Some(pr.id.clone()),
                                ..CiJobQuery::default()
                            },
                        )
                        .await
                    {
                        Ok(jobs) => {
                            for job in jobs {
                                out.push_str(&format!(
                                    "  job {} status={:?} conclusion={:?} created={}\n",
                                    job.name, job.status, job.conclusion, job.created_at
                                ));
                            }
                        }
                        Err(error) => out.push_str(&format!("  list_ci_jobs error: {error}\n")),
                    }
                }
            }
            Err(error) => out.push_str(&format!("list_pull_requests error: {error}\n")),
        }
        out
    })
}

pub(super) fn issue_evidence(issue: &Issue) -> IssueEvidence {
    IssueEvidence {
        number: issue.number.get(),
        title: issue.title.clone(),
        state: issue_state_evidence(issue.state).to_string(),
        labels: issue.labels.clone(),
    }
}

pub(super) fn pr_evidence(pr: &PullRequest) -> PullRequestEvidence {
    PullRequestEvidence {
        number: pr.number.get(),
        title: pr.title.clone(),
        state: pr_state_evidence(pr.state).to_string(),
        labels: pr.labels.clone(),
        author: pr.author_id.to_string(),
        merged_by: pr.merge.as_ref().map(|merge| merge.merged_by.to_string()),
        head_branch: pr.source.branch.clone(),
        head_sha: pr.head_sha.clone(),
        merged_sha: pr.merge.as_ref().map(|merge| merge.commit_sha.clone()),
    }
}

pub(super) fn ci_job_evidence(job: &CiJob) -> CiJobEvidence {
    CiJobEvidence {
        name: job.name.clone(),
        status: format!("{:?}", job.status),
        conclusion: job.conclusion.map(|conclusion| format!("{conclusion:?}")),
        url: job.url.clone(),
    }
}

pub(super) fn require_labels(labels: &[String], required: &[&str]) -> Result<(), String> {
    for required in required {
        if !labels.iter().any(|label| label == required) {
            return Err(format!("missing label `{required}` from {labels:?}"));
        }
    }
    Ok(())
}

pub(super) fn reject_labels(labels: &[String], rejected: &[&str]) -> Result<(), String> {
    for rejected in rejected {
        if labels.iter().any(|label| label == rejected) {
            return Err(format!("unexpected label `{rejected}` in {labels:?}"));
        }
    }
    Ok(())
}

pub(super) fn poll_until<T>(
    deadline: Instant,
    standalone: &mut ChildGuard,
    mut assert: impl FnMut() -> Result<T, String>,
) -> Result<T, String> {
    loop {
        if let Some(status) = standalone.try_wait()? {
            return Err(format!("{} exited early with {status:?}", standalone.label));
        }
        match assert() {
            Ok(value) => return Ok(value),
            Err(error) if Instant::now() < deadline => {
                std::thread::sleep(ASSERT_POLL);
                let _ = error;
            }
            Err(error) => return Err(error),
        }
    }
}

fn pr_state_evidence(state: PullRequestState) -> &'static str {
    match state {
        PullRequestState::Open => "open",
        PullRequestState::Closed => "closed",
        PullRequestState::Merged => "merged",
    }
}

fn issue_state_evidence(state: IssueState) -> &'static str {
    match state {
        IssueState::Open => "open",
        IssueState::Closed => "closed",
    }
}
