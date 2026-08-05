// SPDX-License-Identifier: MPL-2.0

//! Validation-grade bulk terminal-history fixture and convergence proof.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::Value;
use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{
    BranchRef, CommitFile, CreateBranch, CreateIssue, CreatePullRequest, CreateRepository,
    ForgeContent, IssueState, ItemNumber, MergeMethod, MergePullRequest, PullRequestState,
    PullRequestUpdateState, RepositoryId, UpdateIssue, UpdatePullRequest, UpsertLabel, UserId,
};
use temper_workflow::{
    ArtifactKindId, Lease, RoleId, WorkflowMetadata, parse_metadata_block, render_metadata_block,
};

use super::convergence::{issue_evidence, pr_evidence};
use super::process::{ChildGuard, engine_block_on};
use super::{
    FinalStateEvidence, LiveTerminalHistoryEvidence, ScenarioBundle, TerminalHistorySeedFixture,
};

pub(super) struct TerminalHistorySeedResult {
    pub(super) actionable_issue: ItemNumber,
    pub(super) evidence: LiveTerminalHistoryEvidence,
}

pub(super) fn seed(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    scenario: &ScenarioBundle,
    fixture: &TerminalHistorySeedFixture,
) -> Result<TerminalHistorySeedResult, String> {
    let issue = scenario.issue(&fixture.actionable_issue_id)?.clone();
    engine_block_on(async {
        ensure_labels(
            forge,
            repository,
            issue
                .labels
                .iter()
                .chain(fixture.inert_issue_labels.iter())
                .chain(fixture.inert_pull_request_labels.iter())
                .map(String::as_str),
        )
        .await?;

        let actionable_issue = forge
            .create_issue(
                repository,
                CreateIssue {
                    title: issue.title,
                    body: issue.body,
                    labels: issue.labels,
                    assignees: Vec::new(),
                },
            )
            .await
            .map_err(|error| format!("create actionable source issue: {error}"))?;
        forge
            .update_issue(
                &actionable_issue.id,
                UpdateIssue {
                    state: Some(IssueState::Closed),
                    ..UpdateIssue::default()
                },
            )
            .await
            .map_err(|error| format!("close actionable source issue: {error}"))?;

        let actionable_pull =
            create_actionable_pull_request(forge, repository, &scenario.repo.default_branch)
                .await?;

        for index in 0..fixture.target_closed_issues {
            let historical = forge
                .create_issue(
                    repository,
                    CreateIssue {
                        title: format!("irrelevant closed workflow issue {index:04}"),
                        body: "Historical workflow state retained intentionally.".to_string(),
                        labels: fixture.inert_issue_labels.clone(),
                        assignees: Vec::new(),
                    },
                )
                .await
                .map_err(|error| format!("create target history issue {index}: {error}"))?;
            forge
                .update_issue(
                    &historical.id,
                    UpdateIssue {
                        state: Some(IssueState::Closed),
                        ..UpdateIssue::default()
                    },
                )
                .await
                .map_err(|error| format!("close target history issue {index}: {error}"))?;
        }

        let mut first_history_pull_request = None;
        for index in 0..fixture.target_closed_pull_requests {
            let pull = create_inert_pull_request(
                forge,
                repository,
                &scenario.repo.default_branch,
                index,
                &fixture.inert_pull_request_labels,
            )
            .await?;
            first_history_pull_request.get_or_insert(pull.number);
        }

        let (sibling_owner, sibling_name) = split_slug(&fixture.sibling_repo_slug)?;
        let sibling = forge
            .create_repository(CreateRepository {
                owner: sibling_owner.to_string(),
                name: sibling_name.to_string(),
                default_branch: scenario.repo.default_branch.clone(),
                description: Some("terminal discovery owner-scope isolation fixture".to_string()),
            })
            .await
            .map_err(|error| format!("create sibling repository: {error}"))?;
        ensure_labels(
            forge,
            &sibling.id,
            fixture.sibling_issue_labels.iter().map(String::as_str),
        )
        .await?;
        for index in 0..fixture.sibling_closed_issues {
            let issue = forge
                .create_issue(
                    &sibling.id,
                    CreateIssue {
                        title: format!("sibling owner-search pressure {index:04}"),
                        body: "This row belongs to the sibling repository.".to_string(),
                        labels: fixture.sibling_issue_labels.clone(),
                        assignees: Vec::new(),
                    },
                )
                .await
                .map_err(|error| format!("create sibling history issue {index}: {error}"))?;
            forge
                .update_issue(
                    &issue.id,
                    UpdateIssue {
                        state: Some(IssueState::Closed),
                        ..UpdateIssue::default()
                    },
                )
                .await
                .map_err(|error| format!("close sibling history issue {index}: {error}"))?;
        }

        let first_history_pull_request = first_history_pull_request.ok_or_else(|| {
            "terminal history fixture created no irrelevant pull requests".to_string()
        })?;
        Ok(TerminalHistorySeedResult {
            actionable_issue: actionable_issue.number,
            evidence: LiveTerminalHistoryEvidence {
                actionable_issue_number: actionable_issue.number.get(),
                actionable_pull_request_number: actionable_pull.number.get(),
                first_history_pull_request_number: first_history_pull_request.get(),
                target_closed_issues: fixture.target_closed_issues,
                target_closed_pull_requests: fixture.target_closed_pull_requests,
                sibling_repo_slug: fixture.sibling_repo_slug.clone(),
                sibling_closed_issues: fixture.sibling_closed_issues,
                webhook_delivery: "omitted: standalone listener offline during seed".to_string(),
                actionable_older_than_history: actionable_pull.number < first_history_pull_request,
                actionable_recovered: false,
                cold_authority_rebuilt: false,
            },
        })
    })
}

async fn create_actionable_pull_request(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    default_branch: &str,
) -> Result<temper_forge_model::PullRequest, String> {
    let branch = "history/actionable-terminal-recovery";
    forge
        .create_branch(
            repository,
            CreateBranch {
                new_branch: branch.to_string(),
                from_branch: default_branch.to_string(),
            },
        )
        .await
        .map_err(|error| format!("create actionable recovery branch: {error}"))?;
    forge
        .commit_file(
            repository,
            CommitFile {
                path: "terminal-history/actionable.txt".to_string(),
                contents: b"old actionable terminal recovery evidence\n".to_vec(),
                message: "test: seed old terminal recovery target".to_string(),
                branch: branch.to_string(),
            },
        )
        .await
        .map_err(|error| format!("commit actionable recovery branch: {error}"))?;
    let timestamp = |value: &str| -> DateTime<Utc> { value.parse().expect("static timestamp") };
    let body = format!(
        "Old merged artifact with a deliberately expired worker lease.\n\n{}",
        render_metadata_block(&WorkflowMetadata {
            kind: Some(ArtifactKindId::new("implementation_pr")),
            lease: Some(Lease {
                role: RoleId::new("architect"),
                worker: "scenario-lost-worker".to_string(),
                claimed_at: timestamp("2020-01-01T00:00:00Z"),
                heartbeat_at: timestamp("2020-01-01T00:01:00Z"),
                expires_at: timestamp("2020-01-01T00:02:00Z"),
            }),
            ..WorkflowMetadata::default()
        })
    );
    let pull = forge
        .create_pull_request(
            repository,
            CreatePullRequest {
                title: "Old actionable terminal recovery target".to_string(),
                body,
                source: BranchRef {
                    repository_id: repository.clone(),
                    branch: branch.to_string(),
                },
                target: BranchRef {
                    repository_id: repository.clone(),
                    branch: default_branch.to_string(),
                },
                labels: vec![
                    "implementation".to_string(),
                    "landed".to_string(),
                    "recovery".to_string(),
                ],
                assignees: Vec::<UserId>::new(),
            },
        )
        .await
        .map_err(|error| format!("create actionable recovery pull request: {error}"))?;
    forge
        .merge_pull_request(
            &pull.id,
            MergePullRequest {
                method: MergeMethod::Squash,
                commit_title: None,
                commit_body: None,
                delete_source_branch: false,
            },
        )
        .await
        .map_err(|error| format!("merge actionable recovery pull request: {error}"))?;
    forge
        .get_pull_request_by_number(repository, pull.number)
        .await
        .map_err(|error| format!("reload actionable recovery pull request: {error}"))?
        .ok_or_else(|| "actionable recovery pull request disappeared".to_string())
}

async fn create_inert_pull_request(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    default_branch: &str,
    index: usize,
    labels: &[String],
) -> Result<temper_forge_model::PullRequest, String> {
    let branch = format!("history/inert-{index:04}");
    forge
        .create_branch(
            repository,
            CreateBranch {
                new_branch: branch.clone(),
                from_branch: default_branch.to_string(),
            },
        )
        .await
        .map_err(|error| format!("create history PR branch {index}: {error}"))?;
    forge
        .commit_file(
            repository,
            CommitFile {
                path: format!("terminal-history/inert-{index:04}.txt"),
                contents: format!("irrelevant historical pull request {index}\n").into_bytes(),
                message: format!("test: seed inert terminal PR {index:04}"),
                branch: branch.clone(),
            },
        )
        .await
        .map_err(|error| format!("commit history PR branch {index}: {error}"))?;
    let pull = forge
        .create_pull_request(
            repository,
            CreatePullRequest {
                title: format!("Irrelevant closed workflow PR {index:04}"),
                body: "Persistent historical labels without recovery evidence.".to_string(),
                source: BranchRef {
                    repository_id: repository.clone(),
                    branch,
                },
                target: BranchRef {
                    repository_id: repository.clone(),
                    branch: default_branch.to_string(),
                },
                labels: labels.to_vec(),
                assignees: Vec::<UserId>::new(),
            },
        )
        .await
        .map_err(|error| format!("create history PR {index}: {error}"))?;
    forge
        .update_pull_request(
            &pull.id,
            UpdatePullRequest {
                state: Some(PullRequestUpdateState::Closed),
                ..UpdatePullRequest::default()
            },
        )
        .await
        .map_err(|error| format!("close history PR {index}: {error}"))
}

async fn ensure_labels<'a>(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    labels: impl Iterator<Item = &'a str>,
) -> Result<(), String> {
    let labels = labels.collect::<BTreeSet<_>>();
    for label in labels {
        forge
            .upsert_label(
                repository,
                UpsertLabel {
                    name: label.to_string(),
                    color: Some("6a737d".to_string()),
                    description: Some("terminal history scenario fixture".to_string()),
                },
            )
            .await
            .map_err(|error| format!("upsert fixture label `{label}`: {error}"))?;
    }
    Ok(())
}

fn split_slug(slug: &str) -> Result<(&str, &str), String> {
    let Some((owner, name)) = slug.split_once('/') else {
        return Err(format!("sibling repository `{slug}` must be owner/name"));
    };
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return Err(format!("sibling repository `{slug}` must be owner/name"));
    }
    Ok((owner, name))
}

pub(super) fn converge(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    actionable_issue: ItemNumber,
    actionable_pull: ItemNumber,
    standalone: &mut ChildGuard,
    timeout: Duration,
    standalone_log: &Path,
) -> Result<FinalStateEvidence, String> {
    let deadline = Instant::now() + timeout;
    super::convergence::poll_until(deadline, standalone, || {
        engine_block_on(async {
            let issue = forge
                .get_issue_by_number(repository, actionable_issue)
                .await
                .map_err(|error| format!("read actionable source issue: {error}"))?
                .ok_or_else(|| "actionable source issue disappeared".to_string())?;
            if issue.state != IssueState::Closed {
                return Err("actionable source issue is not closed".to_string());
            }
            let pull = forge
                .get_pull_request_by_number(repository, actionable_pull)
                .await
                .map_err(|error| format!("read actionable terminal PR: {error}"))?
                .ok_or_else(|| "actionable terminal PR disappeared".to_string())?;
            if pull.state != PullRequestState::Merged {
                return Err(format!(
                    "actionable terminal PR is not merged: {:?}",
                    pull.state
                ));
            }
            let metadata = parse_metadata_block(&pull.body)
                .map_err(|error| format!("parse actionable recovery metadata: {error}"))?
                .ok_or_else(|| "actionable terminal PR lost its metadata".to_string())?;
            if metadata.lease.is_some() {
                return Err("expired actionable lease has not been recovered".to_string());
            }
            let (recovered, cold) = recovery_observations(standalone_log);
            if !recovered || !cold {
                return Err(format!(
                    "structured recovery evidence incomplete: actionable_recovered={recovered} cold_authority_rebuilt={cold}"
                ));
            }
            Ok(FinalStateEvidence {
                issue: issue_evidence(&issue),
                pull_request: pr_evidence(&pull),
                ci_jobs: Vec::new(),
                ci_observations: Vec::new(),
                ci_heads: Vec::new(),
            })
        })
    })
}

pub(super) fn recovery_observations(standalone_log: &Path) -> (bool, bool) {
    let mut logs = vec![standalone_log.to_path_buf()];
    if let Some(parent) = standalone_log.parent() {
        if let Ok(entries) = fs::read_dir(parent) {
            logs.extend(
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| {
                                name.starts_with("standalone.before-restart-")
                                    && name.ends_with(".log")
                            })
                    }),
            );
        }
    }
    let records = logs
        .iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .flat_map(|source| source.lines().map(str::to_string).collect::<Vec<_>>())
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .filter_map(|record| record.get("fields").cloned())
        .filter(|fields| {
            fields.get("measurement").and_then(Value::as_str) == Some("candidate.discovery")
        })
        .collect::<Vec<_>>();
    let actionable_recovered = records.iter().any(|fields| {
        fields
            .get("candidate.retained_row_count")
            .and_then(Value::as_u64)
            .is_some_and(|count| count >= 1)
            && fields
                .get("candidate.hydrated_artifact_count")
                .and_then(Value::as_u64)
                .is_some_and(|count| count >= 1)
    });
    let current = fs::read_to_string(standalone_log).unwrap_or_default();
    let current_records = current
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|record| record.get("fields").cloned())
        .filter(|fields| {
            fields.get("measurement").and_then(Value::as_str) == Some("candidate.discovery")
        })
        .collect::<Vec<_>>();
    let cold_started = current_records.iter().any(|fields| {
        fields
            .get("candidate.discovery_cache_reused")
            .and_then(Value::as_bool)
            == Some(false)
    });
    let authority_complete = current_records.iter().any(|fields| {
        fields
            .get("candidate.discovery_complete")
            .and_then(Value::as_bool)
            == Some(true)
    });
    (actionable_recovered, cold_started && authority_complete)
}
