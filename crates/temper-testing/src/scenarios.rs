//! Backend-neutral scenarios for the reference-delivery runner world.

use chrono::{DateTime, Utc};
use serde_json::json;
use temper_forge::{
    CiJobConclusion, CiJobQuery, CiJobStatus, CreateIssue, Forge, Issue, IssueQuery, IssueState,
    ItemNumber, PullRequest, PullRequestQuery, PullRequestState, Repository, RepositoryId,
    RepositoryQuery, ReviewDecision,
};
use temper_runner::{scan, BoxError, Scenario};
use temper_workflow::{parse_metadata_block, ArtifactRef, CiStatus, QueueId};

use crate::agents::ARCHITECT_PLAN_BEGIN;
use crate::workflow;

const DEPENDENCY_A_TITLE: &str = "Implement prerequisite A";
const DEPENDENCY_B_TITLE: &str = "Implement dependent B";
const CROSS_REPO_PARENT_TITLE: &str = "Ship cross-repo reference delivery";
const CROSS_REPO_SOURCE_CHILD_TITLE: &str = "Implement service-side cross-repo work";
const CROSS_REPO_TARGET_CHILD_TITLE: &str = "Implement canary-side cross-repo work";

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

/// Variant: one intake in repo A fans out into code children in repos A and B.
pub fn cross_repo_fanout_converges() -> Scenario {
    Scenario::new(
        "cross-repo fan-out converges",
        Box::new(|forge, repo| Box::pin(seed_cross_repo_fanout(forge, repo))),
        Box::new(|forge, repo| Box::pin(assert_cross_repo_fanout(forge, repo))),
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

async fn seed_cross_repo_fanout(forge: &dyn Forge, repo: &RepositoryId) -> Result<(), BoxError> {
    let targets = cross_repo_targets(forge, repo).await?;
    let body = format!(
        "A human asks for one change that must be implemented in both `{}` and `{}`.\n\n{}\n{}\n-->",
        repo_display(&targets.source),
        repo_display(&targets.target),
        ARCHITECT_PLAN_BEGIN,
        json!({
            "children": [
                {
                    "slug": "service",
                    "target_repo": targets.source.id,
                    "title": CROSS_REPO_SOURCE_CHILD_TITLE,
                    "body": "Implement the service-side part of the cross-repo change."
                },
                {
                    "slug": "canary",
                    "target_repo": targets.target.id,
                    "title": CROSS_REPO_TARGET_CHILD_TITLE,
                    "body": "Implement the canary-side part of the cross-repo change."
                }
            ]
        })
    );
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: CROSS_REPO_PARENT_TITLE.into(),
                body,
                labels: vec!["untriaged".into()],
                assignees: Vec::new(),
            },
        )
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
    // Forgejo stores review and merge timestamps at second precision, so a
    // review immediately followed by a merge may compare equal even though the
    // merge gate observed the approval first. Only a strictly earlier merge is a
    // premature merge.
    if merge.merged_at < latest_review.submitted_at {
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
    if dependent.state != IssueState::Closed {
        return Err(boxed_error(
            "dependent issue was not explicitly closed after its PR landed",
        ));
    }
    assert_no_in_progress(prerequisite, "prerequisite issue")?;
    assert_no_in_progress(dependent, "dependent issue")?;

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

async fn assert_cross_repo_fanout(forge: &dyn Forge, repo: &RepositoryId) -> Result<(), BoxError> {
    let targets = cross_repo_targets(forge, repo).await?;
    let source_issues = forge
        .list_issues(&targets.source.id, IssueQuery::default())
        .await?;
    let target_issues = forge
        .list_issues(&targets.target.id, IssueQuery::default())
        .await?;
    let parent = issue_by_title(&source_issues, CROSS_REPO_PARENT_TITLE)?;
    let source_child = issue_by_title(&source_issues, CROSS_REPO_SOURCE_CHILD_TITLE)?;
    let target_child = issue_by_title(&target_issues, CROSS_REPO_TARGET_CHILD_TITLE)?;

    assert_closed(source_child, &targets.source, "source child issue")?;
    assert_closed(target_child, &targets.target, "target child issue")?;
    assert_closed(parent, &targets.source, "parent intake issue")?;

    let parent_metadata = parse_metadata_block(&parent.body)?
        .ok_or_else(|| boxed_error("parent intake issue is missing workflow metadata"))?;
    let source_ref = ArtifactRef::in_repo(targets.source.id.clone(), source_child.number);
    let target_ref = ArtifactRef::in_repo(targets.target.id.clone(), target_child.number);
    if !parent_metadata.dependencies.contains(&source_ref)
        || !parent_metadata.dependencies.contains(&target_ref)
    {
        return Err(boxed_error(format!(
            "parent dependencies did not include both children: expected {}#{} and {}#{}, got {:?}",
            repo_display(&targets.source),
            source_child.number,
            repo_display(&targets.target),
            target_child.number,
            parent_metadata.dependencies
        )));
    }

    let source_prs = implementation_prs(forge, &targets.source.id).await?;
    let target_prs = implementation_prs(forge, &targets.target.id).await?;
    let source_child_pr = implementation_pr_for_parent(&source_prs, source_child.number)?;
    let parent_pr = implementation_pr_for_parent(&source_prs, parent.number)?;
    let target_child_pr = implementation_pr_for_parent(&target_prs, target_child.number)?;
    assert_pr_merged_and_reconciled(source_child_pr)?;
    assert_pr_merged_and_reconciled(target_child_pr)?;
    assert_pr_merged_and_reconciled(parent_pr)?;

    let latest_child_closed = source_child
        .closed_at
        .into_iter()
        .chain(target_child.closed_at)
        .max()
        .ok_or_else(|| boxed_error("closed children are missing closed_at timestamps"))?;
    let parent_closed = parent
        .closed_at
        .ok_or_else(|| boxed_error("closed parent issue is missing closed_at"))?;
    if parent_closed < latest_child_closed || parent_pr.created_at < latest_child_closed {
        return Err(boxed_error(format!(
            "parent in {} resolved before both children landed (latest child closed at {latest_child_closed}, parent PR created at {}, parent closed at {parent_closed})",
            repo_display(&targets.source),
            parent_pr.created_at
        )));
    }

    assert_quiescent(forge, &targets.source.id).await?;
    assert_quiescent(forge, &targets.target.id).await
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
    let now = quiescence_now(forge, repo).await?;
    let work_items = scan(forge, repo, &workflow, &compiled, now).await?;
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

fn assert_closed(issue: &Issue, repo: &Repository, label: &str) -> Result<(), BoxError> {
    if issue.state != IssueState::Closed {
        return Err(boxed_error(format!(
            "{label} {}#{} was not closed (state: {:?}, labels: {:?})",
            repo_display(repo),
            issue.number,
            issue.state,
            issue.labels
        )));
    }
    assert_no_in_progress(issue, label)
}

fn assert_no_in_progress(issue: &Issue, label: &str) -> Result<(), BoxError> {
    if has_label(&issue.labels, "in-progress") {
        return Err(boxed_error(format!(
            "{label} #{} is complete but still has in-progress: {:?}",
            issue.number, issue.labels
        )));
    }
    Ok(())
}

struct CrossRepoTargets {
    source: Repository,
    target: Repository,
}

async fn cross_repo_targets(
    forge: &dyn Forge,
    source_repo: &RepositoryId,
) -> Result<CrossRepoTargets, BoxError> {
    let source = forge
        .get_repository(source_repo)
        .await?
        .ok_or_else(|| boxed_error(format!("source repository {source_repo} was not found")))?;
    let mut repositories = forge.list_repositories(RepositoryQuery::default()).await?;
    repositories.sort_by(|left, right| {
        (left.owner.as_str(), left.name.as_str(), &left.id).cmp(&(
            right.owner.as_str(),
            right.name.as_str(),
            &right.id,
        ))
    });
    let target = repositories
        .into_iter()
        .find(|candidate| candidate.id != *source_repo)
        .ok_or_else(|| {
            boxed_error(format!(
                "cross-repo scenario needs a second repository visible from {}",
                repo_display(&source)
            ))
        })?;
    Ok(CrossRepoTargets { source, target })
}

fn repo_display(repo: &Repository) -> String {
    format!("{}/{}", repo.owner, repo.name)
}

fn parent_numbers(pull_request: &PullRequest) -> Result<Vec<ItemNumber>, BoxError> {
    let metadata = parse_metadata_block(&pull_request.body)?
        .ok_or_else(|| boxed_error("implementation PR is missing workflow metadata"))?;
    Ok(metadata
        .parents
        .into_iter()
        .filter(|parent| parent.is_same_repo())
        .map(|parent| parent.number)
        .collect())
}

/// A backend-neutral "now" for the quiescence scan.
///
/// `owner_alignment` activates on `min_depth >= 5` **or** an oldest member older
/// than `max_age` (7 days). The quiescence assert checks that a small, *fresh*
/// cohort does **not** activate it, which is a `min_depth` fact — so `now` must be
/// recent relative to the artifacts' timestamps on whichever backend is under
/// test, or `max_age` mis-fires and the assert wrongly sees activation.
///
/// A fixed epoch constant only worked because the filesystem/memory backends date
/// artifacts near the epoch too; on real Forgejo (wall-clock ~2026) an epoch `now`
/// makes ages negative, and a wall-clock `now` against epoch-dated filesystem
/// artifacts makes ages ~decades — both inconsistent. Deriving `now` from the
/// artifacts themselves (the latest `updated_at` observed, which `queue_active`
/// ages against) keeps every member's age ≈ 0 on **either** backend, so the assert
/// always exercises the intended `min_depth` behavior. Falls back to the Unix
/// epoch when the repository has no timestamped artifacts (an empty scan is
/// trivially quiescent regardless of `now`).
async fn quiescence_now(forge: &dyn Forge, repo: &RepositoryId) -> Result<DateTime<Utc>, BoxError> {
    let mut latest = DateTime::<Utc>::from_timestamp(0, 0).expect("Unix epoch is valid");
    for issue in forge.list_issues(repo, IssueQuery::default()).await? {
        latest = latest.max(issue.updated_at);
    }
    for pull_request in forge
        .list_pull_requests(repo, PullRequestQuery::default())
        .await?
    {
        latest = latest.max(pull_request.updated_at);
    }
    Ok(latest)
}

fn has_label(labels: &[String], needle: &str) -> bool {
    labels.iter().any(|label| label == needle)
}

fn boxed_error(message: impl Into<String>) -> BoxError {
    Box::new(std::io::Error::other(message.into()))
}
