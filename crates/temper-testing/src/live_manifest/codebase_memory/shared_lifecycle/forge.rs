//! Real Forgejo mutation is the only cancellation stimulus.
use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{Issue, IssueState, ItemNumber, RepositoryId, UpdateIssue};
use temper_workflow::{parse_metadata_block, replace_metadata_block};

use crate::live_manifest::process::engine_block_on;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Attempt {
    pub worker: String,
    pub job: String,
    pub attempt: String,
    pub boot: String,
}

pub(super) fn issue(
    forge: &ForgejoForge,
    repo: &RepositoryId,
    number: ItemNumber,
) -> Result<Issue, String> {
    engine_block_on(forge.get_issue_by_number(repo, number))
        .map_err(|error| format!("read lifecycle issue: {error}"))?
        .ok_or_else(|| "lifecycle issue disappeared".into())
}

pub(super) fn attempt(issue: &Issue) -> Result<Attempt, String> {
    let metadata = parse_metadata_block(&issue.body)
        .map_err(|_| "invalid lifecycle issue metadata")?
        .ok_or("missing lifecycle metadata")?;
    let assignment = metadata
        .assignment
        .ok_or("lifecycle issue not assigned yet")?;
    Ok(Attempt {
        worker: assignment.worker_id.ok_or("missing worker")?,
        job: assignment.job_id.ok_or("missing job")?,
        attempt: assignment.attempt_id.ok_or("missing attempt")?,
        boot: assignment.daemon_boot_id.ok_or("missing boot")?,
    })
}

pub(super) fn activate(forge: &ForgejoForge, current: &Issue) -> Result<(), String> {
    engine_block_on(forge.update_issue(
        &current.id,
        UpdateIssue {
            add_labels: vec!["ready".into()],
            expected_version: Some(current.version),
            ..Default::default()
        },
    ))
    .map_err(|error| format!("activate B: {error}"))?;
    Ok(())
}

pub(super) fn revoke(
    forge: &ForgejoForge,
    current: &Issue,
    expected: &Attempt,
) -> Result<Issue, String> {
    if &attempt(current)? != expected {
        return Err("A changed attempt before withdrawal".into());
    }
    let mut metadata = parse_metadata_block(&current.body)
        .map_err(|_| "invalid A metadata")?
        .ok_or("missing A metadata")?;
    metadata.assignment = None;
    metadata.lease = None;
    let body = replace_metadata_block(&current.body, &metadata)
        .map_err(|_| "cannot replace A metadata")?;
    engine_block_on(forge.update_issue(
        &current.id,
        UpdateIssue {
            body: Some(body),
            state: Some(IssueState::Closed),
            remove_labels: vec!["ready".into(), "in-progress".into()],
            expected_version: Some(current.version),
            ..Default::default()
        },
    ))
    .map_err(|error| format!("withdraw exact A assignment: {error}"))
}

pub(super) fn unchanged_withdrawal(expected: &Issue, current: &Issue) -> Result<(), String> {
    if current.state != IssueState::Closed
        || current.body != expected.body
        || current.labels != expected.labels
    {
        return Err("late A authority changed its withdrawn issue".into());
    }
    Ok(())
}
