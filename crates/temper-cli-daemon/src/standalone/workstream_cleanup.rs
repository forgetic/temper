// SPDX-License-Identifier: MPL-2.0

#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

use temper_engine::{Daemon, PullRequestMergeObserver};
use temper_forge::{PullRequest, PullRequestState};

const ENGINEER_WORKSTREAM_ROLE: &str = "engineer";

#[derive(Clone, Debug, Eq, PartialEq)]
enum MergedPullRequestWorkstream {
    Cleanup { correlation_key: String },
    SkipUnmerged,
    SkipMissingCorrelationKey,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum StandaloneWorkstreamCleanupOutcome {
    SkippedUnmerged,
    SkippedMissingCorrelationKey,
    Workspace(temper_worker::ScopedWorkspaceCleanupOutcome),
}

pub(super) struct StandaloneWorkstreamCleaner {
    daemon: Daemon,
    workspace_root: PathBuf,
}

impl StandaloneWorkstreamCleaner {
    pub(super) fn new(daemon: Daemon, workspace_root: PathBuf) -> Self {
        Self {
            daemon,
            workspace_root,
        }
    }
}

#[async_trait::async_trait]
impl PullRequestMergeObserver for StandaloneWorkstreamCleaner {
    async fn pull_request_merged(&self, pull_request: &PullRequest) {
        match cleanup_engineer_workstream_for_merged_pr(
            &self.daemon,
            self.workspace_root.clone(),
            pull_request,
        )
        .await
        {
            Ok(outcome) => {
                tracing::debug!(
                    target: "temper::worker",
                    repo = pull_request.repo_id.as_str(),
                    pull_request = pull_request.number.get(),
                    ?outcome,
                    "standalone workstream cleanup after PR merge"
                );
            }
            Err(error) => {
                tracing::warn!(
                    target: "temper::worker",
                    repo = pull_request.repo_id.as_str(),
                    pull_request = pull_request.number.get(),
                    %error,
                    "standalone workstream cleanup after PR merge failed"
                );
            }
        }
    }
}

async fn cleanup_engineer_workstream_for_merged_pr(
    daemon: &Daemon,
    workspace_root: PathBuf,
    pull_request: &PullRequest,
) -> Result<StandaloneWorkstreamCleanupOutcome, String> {
    let correlation_key = match merged_pull_request_workstream(pull_request)
        .map_err(|error| format!("parse pull request workflow metadata: {error}"))?
    {
        MergedPullRequestWorkstream::Cleanup { correlation_key } => correlation_key,
        MergedPullRequestWorkstream::SkipUnmerged => {
            return Ok(StandaloneWorkstreamCleanupOutcome::SkippedUnmerged);
        }
        MergedPullRequestWorkstream::SkipMissingCorrelationKey => {
            return Ok(StandaloneWorkstreamCleanupOutcome::SkippedMissingCorrelationKey);
        }
    };
    let active = daemon
        .workstream_active_by_correlation_key(&correlation_key)
        .await;
    let outcome = temper_worker::cleanup_scoped_workspace(
        workspace_root,
        ENGINEER_WORKSTREAM_ROLE.to_string(),
        correlation_key,
        active,
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(StandaloneWorkstreamCleanupOutcome::Workspace(outcome))
}

#[cfg(test)]
fn cleanup_engineer_workstream_for_merged_pr_sync(
    workspace_root: &Path,
    pull_request: &PullRequest,
    active: bool,
) -> Result<StandaloneWorkstreamCleanupOutcome, String> {
    let correlation_key = match merged_pull_request_workstream(pull_request)
        .map_err(|error| format!("parse pull request workflow metadata: {error}"))?
    {
        MergedPullRequestWorkstream::Cleanup { correlation_key } => correlation_key,
        MergedPullRequestWorkstream::SkipUnmerged => {
            return Ok(StandaloneWorkstreamCleanupOutcome::SkippedUnmerged);
        }
        MergedPullRequestWorkstream::SkipMissingCorrelationKey => {
            return Ok(StandaloneWorkstreamCleanupOutcome::SkippedMissingCorrelationKey);
        }
    };
    let outcome = temper_worker::cleanup_scoped_workspace_sync(
        workspace_root,
        ENGINEER_WORKSTREAM_ROLE,
        &correlation_key,
        active,
    )
    .map_err(|error| error.to_string())?;
    Ok(StandaloneWorkstreamCleanupOutcome::Workspace(outcome))
}

fn merged_pull_request_workstream(
    pull_request: &PullRequest,
) -> Result<MergedPullRequestWorkstream, temper_workflow::MetadataError> {
    if pull_request.state != PullRequestState::Merged || pull_request.merge.is_none() {
        return Ok(MergedPullRequestWorkstream::SkipUnmerged);
    }
    let Some(metadata) = temper_workflow::parse_metadata_block(&pull_request.body)? else {
        return Ok(MergedPullRequestWorkstream::SkipMissingCorrelationKey);
    };
    let Some(correlation_key) = metadata
        .correlation_key
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
    else {
        return Ok(MergedPullRequestWorkstream::SkipMissingCorrelationKey);
    };
    Ok(MergedPullRequestWorkstream::Cleanup { correlation_key })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use temper_forge::{
        BranchRef, ItemNumber, MergeMethod, MergeRecord, PullRequestId, RepositoryId, UserId,
        Version,
    };
    use temper_workflow::{WorkflowMetadata, render_metadata_block};

    #[test]
    fn standalone_cleanup_removes_merged_engineer_workstream() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = workstream_path(&temp);
        std::fs::create_dir_all(path.join("temper")).expect("workspace dir");
        let store = save_session(&temp);

        let outcome = cleanup_engineer_workstream_for_merged_pr_sync(
            temp.path(),
            &pull_request(PullRequestState::Merged, Some("pr-for-code-477"), true),
            false,
        )
        .expect("cleanup");

        assert_eq!(
            outcome,
            StandaloneWorkstreamCleanupOutcome::Workspace(
                temper_worker::ScopedWorkspaceCleanupOutcome::Removed { path: path.clone() }
            )
        );
        assert!(!path.exists());
        assert_eq!(store.load_sync().expect("session removed"), None);
    }

    #[test]
    fn standalone_cleanup_skips_open_and_closed_unmerged_pull_requests() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = workstream_path(&temp);
        std::fs::create_dir_all(&path).expect("workspace dir");

        for state in [PullRequestState::Open, PullRequestState::Closed] {
            let outcome = cleanup_engineer_workstream_for_merged_pr_sync(
                temp.path(),
                &pull_request(state, Some("pr-for-code-477"), false),
                false,
            )
            .expect("cleanup");
            assert_eq!(outcome, StandaloneWorkstreamCleanupOutcome::SkippedUnmerged);
            assert!(path.exists());
        }
    }

    #[test]
    fn standalone_cleanup_skips_missing_metadata_or_correlation_key() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = workstream_path(&temp);
        std::fs::create_dir_all(&path).expect("workspace dir");

        let no_metadata = cleanup_engineer_workstream_for_merged_pr_sync(
            temp.path(),
            &pull_request_with_body(PullRequestState::Merged, "human body", true),
            false,
        )
        .expect("cleanup");
        assert_eq!(
            no_metadata,
            StandaloneWorkstreamCleanupOutcome::SkippedMissingCorrelationKey
        );
        assert!(path.exists());

        let empty_key = cleanup_engineer_workstream_for_merged_pr_sync(
            temp.path(),
            &pull_request(PullRequestState::Merged, Some("  "), true),
            false,
        )
        .expect("cleanup");
        assert_eq!(
            empty_key,
            StandaloneWorkstreamCleanupOutcome::SkippedMissingCorrelationKey
        );
        assert!(path.exists());
    }

    #[test]
    fn standalone_cleanup_preserves_active_workstream() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = workstream_path(&temp);
        std::fs::create_dir_all(&path).expect("workspace dir");
        let store = save_session(&temp);

        let outcome = cleanup_engineer_workstream_for_merged_pr_sync(
            temp.path(),
            &pull_request(PullRequestState::Merged, Some("pr-for-code-477"), true),
            true,
        )
        .expect("cleanup");

        assert_eq!(
            outcome,
            StandaloneWorkstreamCleanupOutcome::Workspace(
                temper_worker::ScopedWorkspaceCleanupOutcome::SkippedActive { path: path.clone() }
            )
        );
        assert!(path.exists());
        assert!(store.load_sync().expect("session preserved").is_some());
    }

    fn workstream_path(temp: &tempfile::TempDir) -> PathBuf {
        temper_worker::scoped_workspace_root(
            temp.path(),
            ENGINEER_WORKSTREAM_ROLE,
            "pr-for-code-477",
        )
        .expect("scoped path")
    }

    fn save_session(temp: &tempfile::TempDir) -> temper_worker::AgentSessionStore {
        let store = temper_worker::AgentSessionStore::for_workspace_root(
            temp.path(),
            ENGINEER_WORKSTREAM_ROLE,
            "pr-for-code-477",
        )
        .expect("session store");
        store
            .save_sync(&temper_protocol_agent::AgentSessionState::new(
                "session-477",
            ))
            .expect("save session");
        store
    }

    fn pull_request(
        state: PullRequestState,
        correlation_key: Option<&str>,
        merged: bool,
    ) -> PullRequest {
        let body = correlation_key
            .map(|correlation_key| {
                render_metadata_block(&WorkflowMetadata {
                    correlation_key: Some(correlation_key.to_string()),
                    ..WorkflowMetadata::default()
                })
            })
            .unwrap_or_default();
        pull_request_with_body(state, &body, merged)
    }

    fn pull_request_with_body(state: PullRequestState, body: &str, merged: bool) -> PullRequest {
        let repo_id = RepositoryId::new("forgejo://ai/temper");
        PullRequest {
            id: PullRequestId::new("pr-477"),
            repo_id: repo_id.clone(),
            number: ItemNumber::new(477),
            title: "Implement cleanup".to_string(),
            body: body.to_string(),
            state,
            author_id: UserId::new("engineer"),
            source: BranchRef {
                repository_id: repo_id.clone(),
                branch: "agent/pr-for-code-477".to_string(),
            },
            target: BranchRef {
                repository_id: repo_id,
                branch: "main".to_string(),
            },
            head_sha: Some("abc123".to_string()),
            base_sha: Some("def456".to_string()),
            labels: Vec::new(),
            assignees: Vec::new(),
            requested_reviewers: Vec::new(),
            dependencies: Vec::new(),
            merge: merged.then(|| MergeRecord {
                method: MergeMethod::Squash,
                commit_sha: "abc123def456".to_string(),
                merged_by: UserId::new("maintainer"),
                merged_at: ts("2026-06-01T00:00:00Z"),
            }),
            version: Version::default(),
            created_at: ts("2026-05-31T00:00:00Z"),
            updated_at: ts("2026-06-01T00:00:00Z"),
            closed_at: merged.then(|| ts("2026-06-01T00:00:00Z")),
        }
    }

    fn ts(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("timestamp parses")
            .with_timezone(&Utc)
    }
}
