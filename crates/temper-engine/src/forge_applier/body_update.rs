// SPDX-License-Identifier: MPL-2.0

//! Optimistic title/body updates for implementation PR finalization.

use temper_forge::{Forge, ForgeError, PullRequest, UpdatePullRequest};
use temper_workflow::WorkflowMetadata;

use crate::InFlightJob;
use crate::forge_applier::ForgeApplier;
use crate::forge_applier::body_merge::{canonical_snapshot_body, merge_implementation_pr_body};

pub(super) struct HandoffUpdateResult {
    pub(super) pull_request: PullRequest,
    pub(super) updated: bool,
}

impl<F: Forge + ?Sized> ForgeApplier<F> {
    /// Updates an ordinary implementation handoff. The desired body's metadata
    /// is only a fallback for legacy PRs with no managed record; each retry
    /// still prefers metadata parsed from that attempt's fresh snapshot.
    pub(super) async fn update_implementation_pr_handoff(
        &self,
        job: &InFlightJob,
        pull_request: PullRequest,
        desired_title: &str,
        desired_body: &str,
        operation: &'static str,
    ) -> HandoffUpdateResult {
        let desired = match canonical_snapshot_body(desired_body) {
            Ok(desired) => desired,
            Err(error) => {
                tracing::warn!(
                    target: "temper_daemon",
                    job_id = %job.job_id,
                    pull_request = %pull_request.number,
                    %error,
                    "forge applier could not separate implementation PR prose and metadata for {operation}"
                );
                return HandoffUpdateResult {
                    pull_request,
                    updated: false,
                };
            }
        };
        self.update_implementation_pr_handoff_parts(
            job,
            pull_request,
            desired_title,
            &desired.prose,
            desired.metadata.as_ref(),
            operation,
        )
        .await
    }

    /// Updates a repair handoff using authored prose only. Workflow metadata is
    /// required to come from the latest Forge snapshot on every CAS attempt.
    pub(super) async fn update_pull_request_repair_handoff_parts(
        &self,
        job: &InFlightJob,
        pull_request: PullRequest,
        desired_title: &str,
        desired_prose: &str,
        operation: &'static str,
    ) -> HandoffUpdateResult {
        self.update_implementation_pr_handoff_parts(
            job,
            pull_request,
            desired_title,
            desired_prose,
            None,
            operation,
        )
        .await
    }

    async fn update_implementation_pr_handoff_parts(
        &self,
        job: &InFlightJob,
        mut pull_request: PullRequest,
        desired_title: &str,
        desired_prose: &str,
        fallback_metadata: Option<&WorkflowMetadata>,
        operation: &'static str,
    ) -> HandoffUpdateResult {
        for _ in 0..3 {
            let title = (pull_request.title != desired_title).then(|| desired_title.to_string());
            let body = match merge_implementation_pr_body(
                &pull_request.body,
                desired_prose,
                fallback_metadata,
            ) {
                Ok(body) => body,
                Err(error) => {
                    tracing::warn!(
                        target: "temper_daemon",
                        job_id = %job.job_id,
                        pull_request = %pull_request.number,
                        %error,
                        "forge applier could not merge implementation PR body for {operation}"
                    );
                    return HandoffUpdateResult {
                        pull_request,
                        updated: false,
                    };
                }
            };

            if title.is_none() && body.is_none() {
                return HandoffUpdateResult {
                    pull_request,
                    updated: false,
                };
            }

            match self
                .forge
                .update_pull_request(
                    &pull_request.id,
                    UpdatePullRequest {
                        title,
                        body,
                        expected_version: Some(pull_request.version),
                        ..UpdatePullRequest::default()
                    },
                )
                .await
            {
                Ok(updated) => {
                    return HandoffUpdateResult {
                        pull_request: updated,
                        updated: true,
                    };
                }
                Err(ForgeError::Conflict(_)) => {
                    match self.forge.get_pull_request(&pull_request.id).await {
                        Ok(Some(reloaded)) => {
                            // The loop intentionally rebuilds the body from
                            // `reloaded`; metadata from the rejected attempt is
                            // never cached across this conflict boundary.
                            pull_request = reloaded;
                            continue;
                        }
                        Ok(None) => {
                            tracing::warn!(
                                target: "temper_daemon",
                                job_id = %job.job_id,
                                pull_request = %pull_request.number,
                                "forge applier could not reload PR after handoff conflict for {operation}"
                            );
                            return HandoffUpdateResult {
                                pull_request,
                                updated: false,
                            };
                        }
                        Err(error) => {
                            tracing::warn!(
                                target: "temper_daemon",
                                job_id = %job.job_id,
                                pull_request = %pull_request.number,
                                %error,
                                "forge applier could not reload PR after handoff conflict for {operation}"
                            );
                            return HandoffUpdateResult {
                                pull_request,
                                updated: false,
                            };
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        target: "temper_daemon",
                        job_id = %job.job_id,
                        pull_request = %pull_request.number,
                        %error,
                        "forge applier could not update implementation PR handoff for {operation}"
                    );
                    return HandoffUpdateResult {
                        pull_request,
                        updated: false,
                    };
                }
            }
        }

        tracing::warn!(
            target: "temper_daemon",
            job_id = %job.job_id,
            pull_request = %pull_request.number,
            "forge applier gave up updating implementation PR handoff after conflicts for {operation}"
        );
        HandoffUpdateResult {
            pull_request,
            updated: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use serde_json::json;
    use temper_forge::{
        BranchRef, CreatePullRequest, CreateRepository, Forge, PullRequestQuery, RepositoryId,
        UpdatePullRequest,
    };
    use temper_forge_memory::MemoryForge;
    use temper_protocol_worker::Artifact;
    use temper_workflow::{
        ArtifactKindId, RawWorkflowSpec, WorkflowMetadata, parse_metadata_block,
        render_metadata_block,
    };

    fn workflow() -> temper_workflow::ValidatedWorkflow {
        let spec: RawWorkflowSpec = serde_json::from_str(include_str!(
            "../../../temper-workflow/fixtures/reference-delivery.json"
        ))
        .expect("reference workflow parses");
        spec.validate().expect("reference workflow validates")
    }

    fn job() -> InFlightJob {
        InFlightJob {
            job_id: "repair-conflict-job".to_string(),
            attempt_id: Some("repair-conflict-attempt".to_string()),
            role: "engineer".to_string(),
            repo: "acme/service".to_string(),
            artifact: Artifact {
                item: json!(1),
                kind: "pull_request".to_string(),
            },
            job_payload: json!({}),
        }
    }

    async fn repository(forge: &MemoryForge) -> RepositoryId {
        forge
            .create_repository(CreateRepository {
                owner: "acme".to_string(),
                name: "service".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repository is created")
            .id
    }

    #[test]
    fn handoff_conflict_recomputes_metadata_from_reloaded_snapshot() {
        temper_engine_io::block_on(async move {
            let forge = Arc::new(MemoryForge::new());
            let repository = repository(&forge).await;
            let initial_metadata = WorkflowMetadata {
                kind: Some(ArtifactKindId::new("implementation_pr")),
                correlation_key: Some("identity-before-conflict".to_string()),
                repaired_head: Some("committed-repair-head".to_string()),
                ..WorkflowMetadata::default()
            };
            let stale_snapshot = forge
                .create_pull_request(
                    &repository,
                    CreatePullRequest {
                        title: "Repair title".to_string(),
                        body: format!(
                            "Old report.\n\n{}",
                            render_metadata_block(&initial_metadata)
                        ),
                        source: BranchRef {
                            repository_id: repository.clone(),
                            branch: "repair-head".to_string(),
                        },
                        target: BranchRef {
                            repository_id: repository.clone(),
                            branch: "main".to_string(),
                        },
                        labels: Vec::new(),
                        assignees: Vec::new(),
                    },
                )
                .await
                .expect("pull request is created");

            let reloaded_metadata = WorkflowMetadata {
                correlation_key: Some("identity-from-reloaded-snapshot".to_string()),
                ..initial_metadata
            };
            forge
                .update_pull_request(
                    &stale_snapshot.id,
                    UpdatePullRequest {
                        body: Some(format!(
                            "Concurrent report.\n\n{}",
                            render_metadata_block(&reloaded_metadata)
                        )),
                        expected_version: Some(stale_snapshot.version),
                        ..UpdatePullRequest::default()
                    },
                )
                .await
                .expect("concurrent snapshot update succeeds");

            let applier = ForgeApplier::new(forge.clone(), Arc::new(workflow()));
            let result = applier
                .update_pull_request_repair_handoff_parts(
                    &job(),
                    stale_snapshot,
                    "Repair title",
                    "# Implementation report\n\nFixed after conflict.",
                    "conflict retry test",
                )
                .await;

            assert!(result.updated, "retry should publish the handoff");
            let pull_request = forge
                .list_pull_requests(&repository, PullRequestQuery::default())
                .await
                .expect("pull requests list")
                .pop()
                .expect("pull request remains");
            assert!(pull_request.body.starts_with("# Implementation report"));
            assert_eq!(
                parse_metadata_block(&pull_request.body).unwrap(),
                Some(reloaded_metadata),
                "retry must use metadata loaded after the expected-version conflict"
            );
            assert!(!pull_request.body.contains("identity-before-conflict"));
        });
    }
}
