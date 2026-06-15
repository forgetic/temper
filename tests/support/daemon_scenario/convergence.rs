use std::time::{Duration, Instant};

use temper_forge::{
    CiJob, CiJobConclusion, CiJobQuery, CiJobStatus, IssueState, ItemNumber, PullRequest,
    PullRequestQuery, PullRequestState, UserId,
};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_testing::forgejo_server::{
    ForgejoServer, Provisioned, RoleIdentity, commit_ci_sentinel,
};
use temper_workflow::{CiStatus, parse_metadata_block};

use super::Variant;
use super::runtime::{block_on, block_on_with_cx};

/// How often the driver re-runs the assert closure while polling.
const ASSERT_POLL: Duration = Duration::from_secs(1);

pub(super) fn drive_variant(
    variant: &Variant,
    server: &ForgejoServer,
    provisioned: &Provisioned,
    engineer: &RoleIdentity,
    forge: &ForgejoForge,
    issue: ItemNumber,
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    match variant.ci_sentinel {
        "present" => poll_until(deadline, || {
            block_on(assert_converged(forge, provisioned, engineer, issue, 1))
        }),
        "deferred" => {
            // Phase A: the worker's marker-less head fails real CI and the PR
            // must stay unmerged while red.
            let head_branch = poll_until(deadline, || {
                block_on(assert_ci_red_and_unmerged(
                    forge,
                    provisioned,
                    engineer,
                    issue,
                ))
            })?;

            // Phase B: push the CI sentinel fix to the PR head as the engineer;
            // the new head goes green and the mechanical backstop lands it.
            {
                let base_url = server.base_url().to_string();
                let token = engineer.token.clone();
                let owner = provisioned.owner.clone();
                let name = provisioned.name.clone();
                let branch = head_branch.clone();
                block_on_with_cx(move |cx| async move {
                    commit_ci_sentinel(&cx, &base_url, &token, &owner, &name, &branch).await
                })
            }
            .map_err(|error| format!("ci sentinel commit failed: {error}"))?;
            eprintln!(
                "daemon_forgejo_e2e scenario '{}' pushed CI sentinel fix to {head_branch}",
                variant.name
            );

            poll_until(deadline, || {
                block_on(assert_converged(forge, provisioned, engineer, issue, 2))
            })
        }
        other => panic!("unknown ci_sentinel variant '{other}'"),
    }
}

/// Full convergence: one merged engineer-authored implementation PR correlated
/// to the seeded issue, green real CI, and the source issue closed.
async fn assert_converged(
    forge: &ForgejoForge,
    provisioned: &Provisioned,
    engineer: &RoleIdentity,
    issue: ItemNumber,
    min_ci_verdicts: usize,
) -> Result<(), String> {
    let pull_request = implementation_pr(forge, provisioned, issue).await?;

    if pull_request.author_id != UserId::new(engineer.user.clone()) {
        return Err(format!(
            "implementation PR #{} was authored by {:?}, not the engineer role identity",
            pull_request.number, pull_request.author_id
        ));
    }

    if pull_request.state != PullRequestState::Merged {
        return Err(format!(
            "implementation PR #{} is not merged (state {:?})",
            pull_request.number, pull_request.state
        ));
    }
    let merge = pull_request
        .merge
        .as_ref()
        .ok_or("merged implementation PR has no merge record")?;
    if merge.merged_by == UserId::new(engineer.user.clone()) {
        return Err(
            "PR was merged by the engineer role identity, not the daemon's mechanical backstop"
                .to_string(),
        );
    }
    if !pull_request.labels.iter().any(|label| label == "landed") {
        return Err("merged implementation PR is missing the landed label".to_string());
    }

    let jobs = completed_ci_jobs(forge, provisioned, &pull_request).await?;
    if jobs.len() < min_ci_verdicts {
        return Err(format!(
            "expected at least {min_ci_verdicts} completed CI verdicts, found {}",
            jobs.len()
        ));
    }
    if min_ci_verdicts >= 2
        && jobs.first().and_then(|job| job.conclusion) != Some(CiJobConclusion::Failure)
    {
        return Err("first CI verdict did not fail".to_string());
    }
    if jobs.last().and_then(|job| job.conclusion) != Some(CiJobConclusion::Success) {
        return Err("latest CI verdict did not pass".to_string());
    }
    if !CiStatus::from_jobs(&jobs).is_passed() {
        return Err("latest CI aggregate is not passing".to_string());
    }

    let issue = forge
        .get_issue_by_number(&provisioned.repository, issue)
        .await
        .map_err(|error| format!("issue lookup failed: {error}"))?
        .ok_or("source issue disappeared")?;
    if issue.state != IssueState::Closed {
        return Err(format!(
            "source issue #{} was not closed on merge (labels {:?})",
            issue.number, issue.labels
        ));
    }

    Ok(())
}

/// Red phase of the CI variant: the engineer-authored PR exists, has at least
/// one failed real CI verdict, and is **not** merged. Returns the head branch.
async fn assert_ci_red_and_unmerged(
    forge: &ForgejoForge,
    provisioned: &Provisioned,
    engineer: &RoleIdentity,
    issue: ItemNumber,
) -> Result<String, String> {
    let pull_request = implementation_pr(forge, provisioned, issue).await?;

    if pull_request.author_id != UserId::new(engineer.user.clone()) {
        return Err(format!(
            "implementation PR #{} was authored by {:?}, not the engineer role identity",
            pull_request.number, pull_request.author_id
        ));
    }
    assert!(
        pull_request.merge.is_none() && pull_request.state == PullRequestState::Open,
        "implementation PR #{} merged or closed while its CI was red (state {:?})",
        pull_request.number,
        pull_request.state
    );

    let jobs = completed_ci_jobs(forge, provisioned, &pull_request).await?;
    if jobs.last().and_then(|job| job.conclusion) != Some(CiJobConclusion::Failure) {
        return Err(format!(
            "no failed CI verdict for PR #{} yet ({} completed jobs)",
            pull_request.number,
            jobs.len()
        ));
    }

    Ok(pull_request.source.branch.clone())
}

/// Finds the single implementation PR and checks its workflow correlation
/// metadata points at the seeded issue.
async fn implementation_pr(
    forge: &ForgejoForge,
    provisioned: &Provisioned,
    issue: ItemNumber,
) -> Result<PullRequest, String> {
    let pull_requests: Vec<PullRequest> = forge
        .list_pull_requests(&provisioned.repository, PullRequestQuery::default())
        .await
        .map_err(|error| format!("list_pull_requests failed: {error}"))?
        .into_iter()
        .filter(|pull_request| {
            pull_request
                .labels
                .iter()
                .any(|label| label == "implementation")
        })
        .collect();
    if pull_requests.len() != 1 {
        return Err(format!(
            "expected exactly one implementation PR, found {}",
            pull_requests.len()
        ));
    }
    let pull_request = pull_requests.into_iter().next().expect("one PR");

    let metadata = parse_metadata_block(&pull_request.body)
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

    Ok(pull_request)
}

async fn completed_ci_jobs(
    forge: &ForgejoForge,
    provisioned: &Provisioned,
    pull_request: &PullRequest,
) -> Result<Vec<CiJob>, String> {
    let mut jobs = forge
        .list_ci_jobs(
            &provisioned.repository,
            CiJobQuery {
                pull_request_id: Some(pull_request.id.clone()),
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

/// Admin-token forge handle with the engineer's web-UI credentials attached for
/// the ADR 0019 CI reads, mirroring the daemon binary's own environment.
pub(super) fn admin_forge(
    server: &ForgejoServer,
    provisioned: &Provisioned,
    engineer: &RoleIdentity,
) -> ForgejoForge {
    ForgejoForge::new(
        ForgejoConfig::new(server.base_url(), &provisioned.admin_token)
            .with_default_repo(&provisioned.owner, &provisioned.name)
            .with_web_ui_credentials(&engineer.user, &engineer.password),
    )
}

/// Lists each PR, its head, and its CI jobs for convergence-failure messages.
pub(super) fn ci_diagnostics(forge: &ForgejoForge, provisioned: &Provisioned) -> String {
    block_on(async {
        let mut out = String::new();
        match forge
            .list_pull_requests(&provisioned.repository, PullRequestQuery::default())
            .await
        {
            Ok(pull_requests) => {
                for pull_request in &pull_requests {
                    out.push_str(&format!(
                        "PR #{} head={} labels={:?} state={:?} merge={}\n",
                        pull_request.number,
                        pull_request.source.branch,
                        pull_request.labels,
                        pull_request.state,
                        pull_request.merge.is_some()
                    ));
                    match forge
                        .list_ci_jobs(
                            &provisioned.repository,
                            CiJobQuery {
                                pull_request_id: Some(pull_request.id.clone()),
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

/// Polls `assert` until it passes or `deadline` elapses, returning the last
/// error on timeout.
fn poll_until<T>(
    deadline: Instant,
    mut assert: impl FnMut() -> Result<T, String>,
) -> Result<T, String> {
    loop {
        let error = match assert() {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        if Instant::now() >= deadline {
            return Err(error);
        }
        std::thread::sleep(ASSERT_POLL);
    }
}
