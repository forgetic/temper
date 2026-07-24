// SPDX-License-Identifier: MPL-2.0

use temper_forge::{
    BranchRef, CiJob, CiJobConclusion, CiJobId, CiJobStatus, CreatePullRequest, CreateRepository,
    Forge, PullRequest, PullRequestUpdateState, UpdatePullRequest, UserId,
};
use temper_forge_memory::{FaultOp, MemoryForge};
use temper_workflow::{
    DurableAssignment, Lease, MissingCiRecoveryState, RawWorkflowSpec, RoleId, WorkflowMetadata,
    parse_metadata_block, render_metadata_block,
};

use super::*;

const WORKFLOW: &str = r#"
{
  "name": "missing-ci-recovery",
  "roles": [{ "id": "engineer", "queues": ["failed"] }],
  "labels": [
    { "id": "implementation" }, { "id": "watch" },
    { "id": "landing" }, { "id": "needs-human" }
  ],
  "artifact_kinds": [
    { "id": "implementation_pr", "target": "pull_request", "identifying_labels": ["implementation"] }
  ],
  "queues": [
    {
      "id": "failed", "artifact": "implementation_pr", "labels": ["watch"],
      "condition": { "kind": "ci_failed" }
    }
  ]
}
"#;

const HEAD: &str = "d87b0965a769b2f871e3a9dc238fdcafefb70378";
const OLD_HEAD: &str = "bcbde4e13771b8e55b13ee9747a5a9b42aa30181";

struct Fixture {
    forge: MemoryForge,
    repository: Repository,
    pull_request: PullRequest,
    workflow: ValidatedWorkflow,
    compiled: CompiledWorkflow,
    now: DateTime<Utc>,
}

impl Fixture {
    async fn new() -> Self {
        let forge = MemoryForge::new();
        let repository = forge
            .create_repository(CreateRepository {
                owner: "ai".into(),
                name: "temper".into(),
                default_branch: "main".into(),
                description: None,
            })
            .await
            .unwrap();
        let body = render_metadata_block(&WorkflowMetadata {
            repaired_head: Some(HEAD.to_string()),
            ..WorkflowMetadata::default()
        });
        let created = forge
            .create_pull_request(
                &repository.id,
                CreatePullRequest {
                    title: "implementation".into(),
                    body,
                    source: BranchRef {
                        repository_id: repository.id.clone(),
                        branch: "agent/pr".into(),
                    },
                    target: BranchRef {
                        repository_id: repository.id.clone(),
                        branch: "main".into(),
                    },
                    labels: vec!["implementation".into(), "landing".into(), "watch".into()],
                    assignees: Vec::<UserId>::new(),
                },
            )
            .await
            .unwrap();
        let pull_request = forge
            .set_pull_request_head(&created.id, Some(HEAD.to_string()))
            .unwrap();
        let workflow = serde_json::from_str::<RawWorkflowSpec>(WORKFLOW)
            .unwrap()
            .validate()
            .unwrap();
        let compiled = workflow.compile();
        Self {
            forge,
            repository,
            pull_request,
            workflow,
            compiled,
            now: "2026-07-21T12:00:00Z".parse().unwrap(),
        }
    }

    fn intent(&self) -> MissingCiRecoveryIntent {
        MissingCiRecoveryIntent {
            expected_head_sha: HEAD.to_string(),
            first_observed_at: "2026-07-21T11:55:00Z".parse().unwrap(),
        }
    }

    async fn recover(&self) -> MissingCiRecoveryOutcome {
        recover_missing_current_head_ci(
            &self.forge,
            &self.repository,
            &self.workflow,
            &self.compiled,
            self.now,
            ArtifactAddress::pull_request(self.pull_request.number),
            &self.intent(),
        )
        .await
    }

    async fn fresh(&self) -> PullRequest {
        self.forge
            .get_pull_request_by_number(&self.repository.id, self.pull_request.number)
            .await
            .unwrap()
            .unwrap()
    }

    async fn set_metadata(&mut self, metadata: WorkflowMetadata) {
        self.pull_request = self
            .forge
            .update_pull_request(
                &self.pull_request.id,
                UpdatePullRequest {
                    body: Some(render_metadata_block(&metadata)),
                    expected_version: Some(self.pull_request.version),
                    ..UpdatePullRequest::default()
                },
            )
            .await
            .unwrap();
    }

    async fn assert_unmutated(&self) {
        let fresh = self.fresh().await;
        assert!(!fresh.labels.iter().any(|label| label == NEEDS_HUMAN_LABEL));
        assert!(
            self.forge
                .list_pull_request_comments(&fresh.id)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

fn assert_retryable(outcome: MissingCiRecoveryOutcome, expected_reason: &str) {
    let MissingCiRecoveryOutcome::Retryable { reason } = outcome else {
        panic!("expected retryable missing-CI recovery, got {outcome:?}");
    };
    assert!(
        reason.contains(expected_reason),
        "retry reason `{reason}` did not contain `{expected_reason}`"
    );
}

fn ci_job(fixture: &Fixture, head: &str, status: CiJobStatus) -> CiJob {
    let completed = (status == CiJobStatus::Completed).then_some(fixture.now);
    CiJob {
        id: CiJobId::new(format!("job-{head}-{status:?}")),
        repo_id: fixture.repository.id.clone(),
        pull_request_id: Some(fixture.pull_request.id.clone()),
        commit_sha: head.to_string(),
        name: "test".into(),
        status,
        conclusion: completed.map(|_| CiJobConclusion::Failure),
        url: None,
        created_at: "2026-07-21T11:59:00Z".parse().unwrap(),
        started_at: (status != CiJobStatus::Queued)
            .then_some("2026-07-21T11:59:10Z".parse().unwrap()),
        completed_at: completed,
        updated_at: fixture.now,
    }
}

#[test]
fn unchanged_ci_gated_unowned_pr_is_parked_once_with_actionable_head_audit() {
    temper_engine_io::block_on(async move {
        let fixture = Fixture::new().await;
        fixture.forge.seed_ci_jobs(
            &fixture.repository.id,
            vec![ci_job(&fixture, OLD_HEAD, CiJobStatus::Completed)],
        );

        assert_eq!(fixture.recover().await, MissingCiRecoveryOutcome::Parked);
        let parked = fixture.fresh().await;
        assert_eq!(
            parked.labels,
            vec!["implementation", "landing", "needs-human", "watch"]
        );
        let comments = fixture
            .forge
            .list_pull_request_comments(&parked.id)
            .await
            .unwrap();
        assert_eq!(comments.len(), 1);
        assert!(
            parse_metadata_block(&parked.body)
                .unwrap()
                .unwrap()
                .missing_ci_recovery
                .is_none(),
            "completed parking clears its transient durable marker"
        );
        let audit = &comments[0].body;
        assert!(audit.contains(HEAD));
        assert!(audit.contains("matching `repaired_head`"));
        assert!(audit.contains("no CI run or status for the current head"));
        assert!(audit.contains("retrigger CI"));
        assert!(audit.contains("clear `needs-human`"));

        let cleared = fixture
            .forge
            .update_pull_request(
                &parked.id,
                UpdatePullRequest {
                    remove_labels: vec![NEEDS_HUMAN_LABEL.to_string()],
                    expected_version: Some(parked.version),
                    ..UpdatePullRequest::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            fixture.recover().await,
            MissingCiRecoveryOutcome::Suppressed
        );
        let comments = fixture
            .forge
            .list_pull_request_comments(&cleared.id)
            .await
            .unwrap();
        assert_eq!(comments.len(), 1, "head marker prevents duplicate audits");
        assert!(
            !fixture
                .fresh()
                .await
                .labels
                .iter()
                .any(|label| label == NEEDS_HUMAN_LABEL),
            "an existing head marker makes restart/replay a complete no-op"
        );
    });
}

#[test]
fn changed_head_and_closed_or_non_ci_gated_prs_are_suppressed() {
    temper_engine_io::block_on(async move {
        let fixture = Fixture::new().await;
        fixture
            .forge
            .set_pull_request_head(&fixture.pull_request.id, Some("new-head".into()))
            .unwrap();
        assert_eq!(
            fixture.recover().await,
            MissingCiRecoveryOutcome::Suppressed
        );
        fixture.assert_unmutated().await;

        let fixture = Fixture::new().await;
        fixture
            .forge
            .update_pull_request(
                &fixture.pull_request.id,
                UpdatePullRequest {
                    state: Some(PullRequestUpdateState::Closed),
                    ..UpdatePullRequest::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            fixture.recover().await,
            MissingCiRecoveryOutcome::Suppressed
        );
        fixture.assert_unmutated().await;

        let fixture = Fixture::new().await;
        fixture
            .forge
            .update_pull_request(
                &fixture.pull_request.id,
                UpdatePullRequest {
                    remove_labels: vec!["watch".into()],
                    ..UpdatePullRequest::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            fixture.recover().await,
            MissingCiRecoveryOutcome::Suppressed
        );
        fixture.assert_unmutated().await;
    });
}

#[test]
fn any_current_head_run_or_job_suppresses_parking() {
    temper_engine_io::block_on(async move {
        assert!(sha_identifies_head(&HEAD[..8], HEAD));
        assert!(!sha_identifies_head(&OLD_HEAD[..8], HEAD));

        let fixture = Fixture::new().await;
        fixture
            .forge
            .seed_ci_run(&fixture.repository.id, Some(&fixture.pull_request.id), HEAD);
        assert_eq!(
            fixture.recover().await,
            MissingCiRecoveryOutcome::Suppressed
        );
        fixture.assert_unmutated().await;

        for status in [
            CiJobStatus::Queued,
            CiJobStatus::Running,
            CiJobStatus::Completed,
        ] {
            let fixture = Fixture::new().await;
            fixture
                .forge
                .seed_ci_jobs(&fixture.repository.id, vec![ci_job(&fixture, HEAD, status)]);
            assert_eq!(
                fixture.recover().await,
                MissingCiRecoveryOutcome::Suppressed
            );
            fixture.assert_unmutated().await;
        }
    });
}

#[test]
fn active_assignment_or_lease_suppresses_parking_but_expired_ownership_does_not() {
    temper_engine_io::block_on(async move {
        let mut fixture = Fixture::new().await;
        fixture
            .set_metadata(WorkflowMetadata {
                repaired_head: Some(HEAD.into()),
                assignment: Some(DurableAssignment {
                    job_id: Some("job-1".into()),
                    role: Some(RoleId::new("engineer")),
                    worker_id: Some("worker-1".into()),
                    expires_at: Some("2026-07-21T12:30:00Z".parse().unwrap()),
                    ..DurableAssignment::default()
                }),
                ..WorkflowMetadata::default()
            })
            .await;
        assert_eq!(
            fixture.recover().await,
            MissingCiRecoveryOutcome::Suppressed
        );
        fixture.assert_unmutated().await;

        let mut fixture = Fixture::new().await;
        fixture
            .set_metadata(WorkflowMetadata {
                repaired_head: Some(HEAD.into()),
                lease: Some(Lease {
                    role: RoleId::new("engineer"),
                    worker: "worker-1".into(),
                    claimed_at: "2026-07-21T11:00:00Z".parse().unwrap(),
                    heartbeat_at: "2026-07-21T11:59:00Z".parse().unwrap(),
                    expires_at: "2026-07-21T12:30:00Z".parse().unwrap(),
                }),
                ..WorkflowMetadata::default()
            })
            .await;
        assert_eq!(
            fixture.recover().await,
            MissingCiRecoveryOutcome::Suppressed
        );
        fixture.assert_unmutated().await;

        let mut fixture = Fixture::new().await;
        fixture
            .set_metadata(WorkflowMetadata {
                repaired_head: Some(HEAD.into()),
                assignment: Some(DurableAssignment {
                    job_id: Some("job-expired".into()),
                    role: Some(RoleId::new("engineer")),
                    worker_id: Some("worker-expired".into()),
                    expires_at: Some("2026-07-21T11:30:00Z".parse().unwrap()),
                    ..DurableAssignment::default()
                }),
                ..WorkflowMetadata::default()
            })
            .await;
        assert_eq!(fixture.recover().await, MissingCiRecoveryOutcome::Parked);
    });
}

#[test]
fn malformed_or_ambiguous_metadata_is_suppressed_and_failed_final_read_retries() {
    temper_engine_io::block_on(async move {
        let fixture = Fixture::new().await;
        fixture
            .forge
            .update_pull_request(
                &fixture.pull_request.id,
                UpdatePullRequest {
                    body: Some("<!-- temper:workflow\n{not-json}\n-->".into()),
                    ..UpdatePullRequest::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            fixture.recover().await,
            MissingCiRecoveryOutcome::Suppressed
        );
        fixture.assert_unmutated().await;

        let mut fixture = Fixture::new().await;
        fixture
            .set_metadata(WorkflowMetadata {
                repaired_head: Some(HEAD.into()),
                assignment: Some(DurableAssignment::default()),
                ..WorkflowMetadata::default()
            })
            .await;
        assert_eq!(
            fixture.recover().await,
            MissingCiRecoveryOutcome::Suppressed
        );
        fixture.assert_unmutated().await;

        let fixture = Fixture::new().await;
        fixture.forge.fail_next(
            FaultOp::GetPullRequestByNumber,
            "injected final validation outage",
        );
        assert_retryable(fixture.recover().await, "pull_request_read_failed");
        fixture.assert_unmutated().await;
        assert_eq!(fixture.recover().await, MissingCiRecoveryOutcome::Parked);
    });
}

#[test]
fn transient_barrier_conflict_and_comment_failure_converge_on_later_passes() {
    temper_engine_io::block_on(async move {
        let fixture = Fixture::new().await;
        fixture.forge.conflict_next(
            FaultOp::UpdatePullRequest,
            "injected conditional parking conflict",
        );
        assert_retryable(fixture.recover().await, "parking_barrier_write_failed");
        fixture.assert_unmutated().await;
        assert_eq!(fixture.recover().await, MissingCiRecoveryOutcome::Parked);

        let fixture = Fixture::new().await;
        fixture.forge.fail_next(
            FaultOp::AddPullRequestComment,
            "injected audit publication outage",
        );
        assert_retryable(
            fixture.recover().await,
            "audit_comment_write_failed_after_barrier",
        );
        let interrupted = fixture.fresh().await;
        assert!(requires_human_attention(&interrupted.labels));
        let interrupted_metadata = parse_metadata_block(&interrupted.body).unwrap().unwrap();
        let operation = interrupted_metadata
            .missing_ci_recovery
            .expect("interrupted parking keeps a durable operation marker");
        assert_eq!(operation.head_sha, HEAD);
        assert!(
            fixture
                .forge
                .list_pull_request_comments(&interrupted.id)
                .await
                .unwrap()
                .is_empty()
        );

        assert_eq!(fixture.recover().await, MissingCiRecoveryOutcome::Parked);
        let completed = fixture.fresh().await;
        assert!(requires_human_attention(&completed.labels));
        assert!(
            parse_metadata_block(&completed.body)
                .unwrap()
                .unwrap()
                .missing_ci_recovery
                .is_none()
        );
        assert_eq!(
            fixture
                .forge
                .list_pull_request_comments(&completed.id)
                .await
                .unwrap()
                .len(),
            1
        );
    });
}

#[test]
fn interrupted_marker_cleanup_retries_without_duplicate_audit() {
    temper_engine_io::block_on(async move {
        let fixture = Fixture::new().await;
        let partial_metadata = WorkflowMetadata {
            repaired_head: Some(HEAD.to_string()),
            missing_ci_recovery: Some(MissingCiRecoveryState {
                head_sha: HEAD.to_string(),
                first_observed_at: fixture.intent().first_observed_at,
            }),
            ..WorkflowMetadata::default()
        };
        fixture
            .forge
            .update_pull_request(
                &fixture.pull_request.id,
                UpdatePullRequest {
                    body: Some(render_metadata_block(&partial_metadata)),
                    add_labels: vec![NEEDS_HUMAN_LABEL.to_string()],
                    expected_version: Some(fixture.pull_request.version),
                    ..UpdatePullRequest::default()
                },
            )
            .await
            .unwrap();

        fixture.forge.conflict_next(
            FaultOp::UpdatePullRequest,
            "injected operation-marker cleanup conflict",
        );
        assert_retryable(fixture.recover().await, "parking_marker_clear_failed");
        let partial_comments = fixture
            .forge
            .list_pull_request_comments(&fixture.pull_request.id)
            .await
            .unwrap();
        assert_eq!(partial_comments.len(), 1);

        assert_eq!(fixture.recover().await, MissingCiRecoveryOutcome::Parked);
        assert_eq!(
            fixture
                .forge
                .list_pull_request_comments(&fixture.pull_request.id)
                .await
                .unwrap()
                .len(),
            1,
            "cleanup retries reuse the head-keyed audit"
        );
        assert!(
            parse_metadata_block(&fixture.fresh().await.body)
                .unwrap()
                .unwrap()
                .missing_ci_recovery
                .is_none()
        );
    });
}

#[test]
fn unrelated_attention_is_not_treated_as_interrupted_missing_ci_parking() {
    temper_engine_io::block_on(async move {
        let fixture = Fixture::new().await;
        fixture
            .forge
            .update_pull_request(
                &fixture.pull_request.id,
                UpdatePullRequest {
                    add_labels: vec![NEEDS_HUMAN_LABEL.to_string()],
                    expected_version: Some(fixture.pull_request.version),
                    ..UpdatePullRequest::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(
            fixture.recover().await,
            MissingCiRecoveryOutcome::Suppressed
        );
        assert!(
            fixture
                .forge
                .list_pull_request_comments(&fixture.pull_request.id)
                .await
                .unwrap()
                .is_empty()
        );
    });
}
