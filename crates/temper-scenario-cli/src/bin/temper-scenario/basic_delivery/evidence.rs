// SPDX-License-Identifier: MPL-2.0

use temper_forge_model::{
    CiJobConclusion, CiJobQuery, CiJobStatus, Forge, Issue, IssueQuery, IssueState, PullRequest,
    PullRequestQuery, PullRequestState, RepositoryId,
};
use temper_runner::BoxError;

use crate::run_evidence;

use super::fixture::{IntakeSeed, RepoSeed};
use super::model::RunEvidence;
use super::state::{ci_job_conclusion_value, has_label, pr_state_evidence};

pub(super) async fn read_evidence(
    forge: &dyn Forge,
    repo: &RepositoryId,
    seed: &IntakeSeed,
    repo_seed: &RepoSeed,
) -> Result<RunEvidence, BoxError> {
    let issues = forge.list_issues(repo, IssueQuery::default()).await?;
    let issue = find_seeded_code_issue(&issues, seed)?;
    let pull_requests = forge
        .list_pull_requests(repo, PullRequestQuery::default())
        .await?;
    let pull_request = only_implementation_pr(&pull_requests)?;
    if pull_request.state != PullRequestState::Merged {
        return Err(boxed_error(format!(
            "implementation PR #{} was not merged (state: {})",
            pull_request.number,
            pr_state_evidence(pull_request.state)
        )));
    }
    if issue.state != IssueState::Closed {
        return Err(boxed_error(format!(
            "seeded code issue #{} was not closed after merge",
            issue.number
        )));
    }
    if has_label(&pull_request.labels, "landing") {
        return Err(boxed_error(format!(
            "implementation PR #{} still has `landing` label",
            pull_request.number
        )));
    }
    for stale_label in ["ready", "untriaged", "in-progress"] {
        if has_label(&issue.labels, stale_label) {
            return Err(boxed_error(format!(
                "seeded code issue #{} still has `{stale_label}` label",
                issue.number
            )));
        }
    }
    if !pull_request
        .body
        .contains(&format!("#{}", issue.number.get()))
    {
        return Err(boxed_error(format!(
            "implementation PR #{} does not reference seeded issue #{}",
            pull_request.number, issue.number
        )));
    }

    let ci_jobs = forge
        .list_ci_jobs(
            repo,
            CiJobQuery {
                pull_request_id: Some(pull_request.id.clone()),
                status: Some(CiJobStatus::Completed),
                ..CiJobQuery::default()
            },
        )
        .await?;
    if !ci_jobs
        .iter()
        .any(|job| job.conclusion == Some(CiJobConclusion::Success))
    {
        return Err(boxed_error(format!(
            "implementation PR #{} has no passing CI job",
            pull_request.number
        )));
    }

    let closed_parent_issues = issues
        .iter()
        .filter(|candidate| candidate.state == IssueState::Closed)
        .filter(|candidate| candidate.number == issue.number)
        .count();
    if closed_parent_issues != 1 {
        return Err(boxed_error(format!(
            "expected exactly 1 closed parent issue, found {closed_parent_issues}"
        )));
    }

    let default_branch_head_sha = pull_request
        .merge
        .as_ref()
        .map(|merge| merge.commit_sha.clone())
        .or_else(|| pull_request.head_sha.clone());

    Ok(RunEvidence {
        issue_number: issue.number,
        issue_title: issue.title.clone(),
        issue_state: issue.state,
        issue_labels: issue.labels.clone(),
        pr_number: pull_request.number,
        pr_title: pull_request.title.clone(),
        pr_state: pull_request.state,
        pr_labels: pull_request.labels.clone(),
        pr_head_branch: pull_request.source.branch.clone(),
        pr_head_sha: pull_request.head_sha.clone(),
        pr_merged_sha: pull_request
            .merge
            .as_ref()
            .map(|merge| merge.commit_sha.clone()),
        repo_id: repo_seed.id.clone(),
        repo_slug: repo_seed.slug.clone(),
        default_branch: repo_seed.default_branch.clone(),
        default_branch_head_sha,
        default_branch_contains_engineer_diff: pull_request.state == PullRequestState::Merged
            && has_label(&pull_request.labels, "implementation"),
        completed_ci_jobs: ci_jobs.len(),
        ci_jobs: ci_jobs
            .iter()
            .map(|job| run_evidence::CiJobEvidence {
                name: job.name.clone(),
                status: format!("{:?}", job.status).to_ascii_lowercase(),
                pull_request_number: Some(pull_request.number.get()),
                conclusion: job
                    .conclusion
                    .map(ci_job_conclusion_value)
                    .map(str::to_string),
                url: job.url.clone(),
            })
            .collect(),
        closed_parent_issues,
    })
}

fn find_seeded_code_issue<'a>(
    issues: &'a [Issue],
    seed: &IntakeSeed,
) -> Result<&'a Issue, BoxError> {
    issues
        .iter()
        .find(|issue| issue.title == seed.title && has_label(&issue.labels, "code"))
        .ok_or_else(|| {
            boxed_error(format!(
                "seeded issue `{}` was not triaged into a code issue",
                seed.title
            ))
        })
}

fn only_implementation_pr(pull_requests: &[PullRequest]) -> Result<&PullRequest, BoxError> {
    let implementation_prs = pull_requests
        .iter()
        .filter(|pull_request| has_label(&pull_request.labels, "implementation"))
        .collect::<Vec<_>>();
    match implementation_prs.as_slice() {
        [pull_request] => Ok(*pull_request),
        [] => Err(boxed_error("no implementation PR was created")),
        many => Err(boxed_error(format!(
            "expected one implementation PR, found {}",
            many.len()
        ))),
    }
}

fn boxed_error(message: impl Into<String>) -> BoxError {
    Box::new(std::io::Error::other(message.into()))
}
