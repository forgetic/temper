//! Backend-neutral scenarios for the reference-delivery runner world.

use chrono::{DateTime, Utc};
use harness_forge::{
    CreateIssue, Forge, IssueQuery, PullRequestQuery, PullRequestState, RepositoryId,
};
use harness_runner::{scan, BoxError, Scenario};
use harness_workflow::{parse_metadata_block, QueueId};

use super::workflow;

/// Happy path: one human-filed request becomes a merged implementation PR.
pub fn happy_path() -> Scenario {
    Scenario::new(
        "reference delivery happy path",
        Box::new(|forge, repo| Box::pin(seed_happy_path(forge, repo))),
        Box::new(|forge, repo| Box::pin(assert_happy_path(forge, repo))),
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

async fn assert_happy_path(forge: &dyn Forge, repo: &RepositoryId) -> Result<(), BoxError> {
    let issues = forge.list_issues(repo, IssueQuery::default()).await?;
    let code_issue = issues
        .iter()
        .find(|issue| has_label(&issue.labels, "code"))
        .ok_or_else(|| boxed_error("triaged code issue was not found"))?;

    let pull_requests = forge
        .list_pull_requests(repo, PullRequestQuery::default())
        .await?;
    let implementation_prs: Vec<_> = pull_requests
        .iter()
        .filter(|pr| has_label(&pr.labels, "implementation"))
        .collect();
    if implementation_prs.len() != 1 {
        return Err(boxed_error(format!(
            "expected exactly one implementation PR, found {}",
            implementation_prs.len()
        )));
    }
    let pull_request = implementation_prs[0];
    let metadata = parse_metadata_block(&pull_request.body)?
        .ok_or_else(|| boxed_error("implementation PR is missing workflow metadata"))?;
    if !metadata.parents.contains(&code_issue.number) {
        return Err(boxed_error(format!(
            "implementation PR #{} does not point at code issue #{}",
            pull_request.number, code_issue.number
        )));
    }
    if pull_request.state != PullRequestState::Merged {
        return Err(boxed_error(format!(
            "implementation PR #{} was not merged",
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

    let workflow = workflow();
    let compiled = workflow.compile();
    let work_items = scan(forge, repo, &workflow, &compiled, scenario_now()).await?;
    let owner_alignment = QueueId::new("owner_alignment");
    if work_items.iter().any(|item| item.queue == owner_alignment) {
        return Err(boxed_error(
            "owner_alignment activated for one fresh PR; expected min_depth 5 to hold it",
        ));
    }
    if !work_items.is_empty() {
        return Err(boxed_error(format!(
            "expected quiescence, found work items: {work_items:?}"
        )));
    }
    Ok(())
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
