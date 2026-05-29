//! Backend-neutral scenarios for the reference-delivery runner world.

use chrono::{DateTime, Utc};
use harness_forge::{
    CiJobConclusion, CiJobQuery, CiJobStatus, CreateIssue, Forge, Issue, IssueQuery, IssueState,
    ItemNumber, PullRequest, PullRequestQuery, PullRequestState, RepositoryId, ReviewDecision,
};
use harness_runner::{scan, BoxError, Scenario};
use harness_workflow::{parse_metadata_block, CiStatus, QueueId};

use super::workflow;

const DEPENDENCY_A_TITLE: &str = "Implement prerequisite A";
const DEPENDENCY_B_TITLE: &str = "Implement dependent B";

/// Happy path: one human-filed request becomes a merged implementation PR.
pub fn happy_path() -> Scenario {
    Scenario::new(
        "reference delivery happy path",
        Box::new(|forge, repo| Box::pin(seed_happy_path(forge, repo))),
        Box::new(|forge, repo| Box::pin(assert_happy_path(forge, repo))),
    )
}

/// Variant: the first review requests changes; a later review approves.
pub fn changes_requested_then_approved() -> Scenario {
    Scenario::new(
        "changes requested then approved",
        Box::new(|forge, repo| Box::pin(seed_happy_path(forge, repo))),
        Box::new(|forge, repo| Box::pin(assert_changes_requested_then_approved(forge, repo))),
    )
}

/// Variant: native CI fails once, engineer handles the failure, then CI passes.
pub fn ci_fails_then_passes() -> Scenario {
    Scenario::new(
        "CI fails then passes",
        Box::new(|forge, repo| Box::pin(seed_happy_path(forge, repo))),
        Box::new(|forge, repo| Box::pin(assert_ci_fails_then_passes(forge, repo))),
    )
}

/// Variant: blocked code work is mechanically unblocked after its dependency lands.
pub fn dependency_chain_mechanically_unblocked() -> Scenario {
    Scenario::new(
        "dependency chain mechanically unblocked",
        Box::new(|forge, repo| Box::pin(seed_dependency_chain(forge, repo))),
        Box::new(|forge, repo| Box::pin(assert_dependency_chain(forge, repo))),
    )
}

async fn seed_happy_path(forge: &dyn Forge, repo: &RepositoryId) -> Result<(), BoxError> {
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: "Ship the end-to-end happy path".into(),
                body: "A human asks the team to implement one small change.".into(),
                labels: vec!["untriaged".into()],
                assignees: Vec::new(),
            },
        )
        .await?;
    Ok(())
}

async fn seed_dependency_chain(forge: &dyn Forge, repo: &RepositoryId) -> Result<(), BoxError> {
    let prerequisite = forge
        .create_issue(
            repo,
            CreateIssue {
                title: DEPENDENCY_A_TITLE.into(),
                body: "A must land before B can start.".into(),
                labels: vec!["code".into(), "ready".into()],
                assignees: Vec::new(),
            },
        )
        .await?;
    let dependent = forge
        .create_issue(
            repo,
            CreateIssue {
                title: DEPENDENCY_B_TITLE.into(),
                body: "B is blocked on A through a native dependency link.".into(),
                labels: vec!["code".into(), "blocked".into()],
                assignees: Vec::new(),
            },
        )
        .await?;
    forge
        .add_issue_dependency(&dependent.id, prerequisite.number)
        .await?;
    Ok(())
}

async fn assert_happy_path(forge: &dyn Forge, repo: &RepositoryId) -> Result<(), BoxError> {
    let issues = forge.list_issues(repo, IssueQuery::default()).await?;
    let code_issue = issues
        .iter()
        .find(|issue| has_label(&issue.labels, "code"))
        .ok_or_else(|| boxed_error("triaged code issue was not found"))?;

    let pull_requests = implementation_prs(forge, repo).await?;
    let pull_request = only_implementation_pr(&pull_requests)?;
    assert_parent(pull_request, code_issue.number)?;
    assert_pr_merged_and_reconciled(pull_request)?;
    assert_quiescent(forge, repo).await
}

async fn assert_changes_requested_then_approved(
    forge: &dyn Forge,
    repo: &RepositoryId,
) -> Result<(), BoxError> {
    let pull_requests = implementation_prs(forge, repo).await?;
    let pull_request = only_implementation_pr(&pull_requests)?;
    assert_pr_merged_and_reconciled(pull_request)?;

    let reviews = forge.list_pull_request_reviews(&pull_request.id).await?;
    if !reviews
        .iter()
        .any(|review| review.decision == ReviewDecision::ChangesRequested)
    {
        return Err(boxed_error("reviewer never requested changes"));
    }
    let latest_review = reviews
        .iter()
        .max_by(|left, right| {
            left.submitted_at
                .cmp(&right.submitted_at)
                .then(left.id.cmp(&right.id))
        })
        .ok_or_else(|| boxed_error("pull request has no reviews"))?;
    if latest_review.decision != ReviewDecision::Approved {
        return Err(boxed_error("latest review was not an approval"));
    }
    let merge = pull_request
        .merge
        .as_ref()
        .ok_or_else(|| boxed_error("merged pull request is missing merge record"))?;
    if merge.merged_at <= latest_review.submitted_at {
        return Err(boxed_error(
            "pull request merged before the approving review was recorded",
        ));
    }

    assert_quiescent(forge, repo).await
}

async fn assert_ci_fails_then_passes(
    forge: &dyn Forge,
    repo: &RepositoryId,
) -> Result<(), BoxError> {
    let pull_requests = implementation_prs(forge, repo).await?;
    let pull_request = only_implementation_pr(&pull_requests)?;
    assert_pr_merged_and_reconciled(pull_request)?;

    let mut jobs = forge
        .list_ci_jobs(
            repo,
            CiJobQuery {
                pull_request_id: Some(pull_request.id.clone()),
                status: Some(CiJobStatus::Completed),
                ..CiJobQuery::default()
            },
        )
        .await?;
    jobs.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then(left.id.cmp(&right.id))
    });
    if jobs.len() < 2 {
        return Err(boxed_error(format!(
            "expected at least two CI verdicts, found {}",
            jobs.len()
        )));
    }
    if jobs.first().and_then(|job| job.conclusion) != Some(CiJobConclusion::Failure) {
        return Err(boxed_error("first CI verdict did not fail"));
    }
    if jobs.last().and_then(|job| job.conclusion) != Some(CiJobConclusion::Success) {
        return Err(boxed_error("latest CI verdict did not pass"));
    }
    if !CiStatus::from_jobs(&jobs).is_passed() {
        return Err(boxed_error("latest CI aggregate is not passing"));
    }

    assert_quiescent(forge, repo).await
}

async fn assert_dependency_chain(forge: &dyn Forge, repo: &RepositoryId) -> Result<(), BoxError> {
    let issues = forge.list_issues(repo, IssueQuery::default()).await?;
    let prerequisite = issue_by_title(&issues, DEPENDENCY_A_TITLE)?;
    let dependent = issue_by_title(&issues, DEPENDENCY_B_TITLE)?;
    if dependent.dependencies != vec![prerequisite.number] {
        return Err(boxed_error(
            "dependent issue does not carry the native dependency link",
        ));
    }
    if prerequisite.state != IssueState::Closed {
        return Err(boxed_error(
            "prerequisite issue was not explicitly closed after its PR landed",
        ));
    }
    if has_label(&dependent.labels, "blocked") {
        return Err(boxed_error(
            "dependent issue was not mechanically unblocked",
        ));
    }
    if !has_label(&dependent.labels, "in-progress") {
        return Err(boxed_error(
            "dependent issue never progressed past the ready state after unblock",
        ));
    }

    let pull_requests = implementation_prs(forge, repo).await?;
    if pull_requests.len() != 2 {
        return Err(boxed_error(format!(
            "expected two implementation PRs, found {}",
            pull_requests.len()
        )));
    }
    let pr_a = implementation_pr_for_parent(&pull_requests, prerequisite.number)?;
    let pr_b = implementation_pr_for_parent(&pull_requests, dependent.number)?;
    assert_pr_merged_and_reconciled(pr_a)?;
    assert_pr_merged_and_reconciled(pr_b)?;

    let closed_at = prerequisite
        .closed_at
        .ok_or_else(|| boxed_error("closed prerequisite issue lacks closed_at"))?;
    if pr_b.created_at <= closed_at {
        return Err(boxed_error(
            "dependent PR was created before the prerequisite issue closed",
        ));
    }

    assert_quiescent(forge, repo).await
}

async fn implementation_prs(
    forge: &dyn Forge,
    repo: &RepositoryId,
) -> Result<Vec<PullRequest>, BoxError> {
    Ok(forge
        .list_pull_requests(repo, PullRequestQuery::default())
        .await?
        .into_iter()
        .filter(|pr| has_label(&pr.labels, "implementation"))
        .collect())
}

fn only_implementation_pr(pull_requests: &[PullRequest]) -> Result<&PullRequest, BoxError> {
    if pull_requests.len() != 1 {
        return Err(boxed_error(format!(
            "expected exactly one implementation PR, found {}",
            pull_requests.len()
        )));
    }
    Ok(&pull_requests[0])
}

fn implementation_pr_for_parent(
    pull_requests: &[PullRequest],
    parent: ItemNumber,
) -> Result<&PullRequest, BoxError> {
    for pull_request in pull_requests {
        if parent_numbers(pull_request)?.contains(&parent) {
            return Ok(pull_request);
        }
    }
    Err(boxed_error(format!(
        "no implementation PR points at parent issue #{parent}"
    )))
}

fn assert_parent(pull_request: &PullRequest, parent: ItemNumber) -> Result<(), BoxError> {
    if !parent_numbers(pull_request)?.contains(&parent) {
        return Err(boxed_error(format!(
            "implementation PR #{} does not point at code issue #{parent}",
            pull_request.number
        )));
    }
    Ok(())
}

fn assert_pr_merged_and_reconciled(pull_request: &PullRequest) -> Result<(), BoxError> {
    if pull_request.state != PullRequestState::Merged {
        return Err(boxed_error(format!(
            "implementation PR #{} was not merged",
            pull_request.number
        )));
    }
    if pull_request.merge.is_none() {
        return Err(boxed_error(format!(
            "implementation PR #{} has no merge record",
            pull_request.number
        )));
    }
    if has_label(&pull_request.labels, "landed") {
        return Err(boxed_error("architect did not clear the landed label"));
    }
    if !has_label(&pull_request.labels, "alignment") {
        return Err(boxed_error(
            "alignment should remain set until the owner cohort activates",
        ));
    }
    if has_label(&pull_request.labels, "needs-merge") {
        return Err(boxed_error("merge routing label was not cleared"));
    }
    Ok(())
}

async fn assert_quiescent(forge: &dyn Forge, repo: &RepositoryId) -> Result<(), BoxError> {
    let workflow = workflow();
    let compiled = workflow.compile();
    let work_items = scan(forge, repo, &workflow, &compiled, scenario_now()).await?;
    let owner_alignment = QueueId::new("owner_alignment");
    if work_items.iter().any(|item| item.queue == owner_alignment) {
        return Err(boxed_error(
            "owner_alignment activated for a small fresh cohort; expected min_depth 5 to hold it",
        ));
    }
    if !work_items.is_empty() {
        return Err(boxed_error(format!(
            "expected quiescence, found work items: {work_items:?}"
        )));
    }
    Ok(())
}

fn issue_by_title<'a>(issues: &'a [Issue], title: &str) -> Result<&'a Issue, BoxError> {
    issues
        .iter()
        .find(|issue| issue.title == title)
        .ok_or_else(|| boxed_error(format!("issue '{title}' was not found")))
}

fn parent_numbers(pull_request: &PullRequest) -> Result<Vec<ItemNumber>, BoxError> {
    let metadata = parse_metadata_block(&pull_request.body)?
        .ok_or_else(|| boxed_error("implementation PR is missing workflow metadata"))?;
    Ok(metadata.parents)
}

fn scenario_now() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(0, 0).expect("Unix epoch is valid")
}

fn has_label(labels: &[String], needle: &str) -> bool {
    labels.iter().any(|label| label == needle)
}

fn boxed_error(message: impl Into<String>) -> BoxError {
    Box::new(std::io::Error::other(message.into()))
}
