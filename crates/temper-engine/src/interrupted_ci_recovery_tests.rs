use super::*;
use temper_forge::{
    BranchRef, CiJob, CiJobConclusion, CiJobId, CiJobStatus, CiRetryOutcome, CreatePullRequest,
    CreateRepository, Forge,
};
use temper_forge_memory::{FaultOp, MemoryForge};
use temper_workflow::{
    ArtifactSource, AssignmentClaimRequest, AssignmentMutation, DurableAssignment, LeaseError,
    LeaseManager, LeasePolicy, RawWorkflowSpec, parse_metadata_block,
};

const HEAD: &str = "abcdef1234567890";

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
                description: None,
                default_branch: "main".into(),
            })
            .await
            .unwrap();
        let pull_request = forge
            .create_pull_request(
                &repository.id,
                CreatePullRequest {
                    title: "Interrupted CI".into(),
                    body: String::new(),
                    source: BranchRef {
                        repository_id: repository.id.clone(),
                        branch: "feature/interrupted".into(),
                    },
                    target: BranchRef {
                        repository_id: repository.id.clone(),
                        branch: "main".into(),
                    },
                    labels: vec!["implementation".into()],
                    assignees: Vec::new(),
                },
            )
            .await
            .unwrap();
        let pull_request = forge
            .set_pull_request_head(&pull_request.id, Some(HEAD.into()))
            .unwrap();
        let now = "2026-07-23T15:38:32Z".parse().unwrap();
        forge.seed_ci_jobs(
            &repository.id,
            vec![job(
                &repository,
                &pull_request,
                "591",
                "1",
                CiJobStatus::Completed,
                Some(CiJobConclusion::RunnerLost),
                now,
            )],
        );
        let workflow = workflow();
        let compiled = workflow.compile();
        Self {
            forge,
            repository,
            pull_request,
            workflow,
            compiled,
            now,
        }
    }

    async fn recover(&self) -> InterruptedCiRecoveryOutcome {
        recover_interrupted_ci(
            &self.forge,
            &self.repository,
            &self.workflow,
            &self.compiled,
            self.now,
            ArtifactAddress::pull_request(self.pull_request.number),
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
}

fn workflow() -> ValidatedWorkflow {
    serde_json::from_str::<RawWorkflowSpec>(
        r#"{
          "name":"interrupted-recovery",
          "roles":[{
            "id":"ci_diagnostician",
            "queues":["pr_ci_recovery"],
            "external_tools":[{"id":"review_workspace","description":"read only"}]
          }],
          "labels":[
            {"id":"implementation"},
            {"id":"needs-human"}
          ],
          "artifact_kinds":[{
            "id":"implementation_pr",
            "target":"pull_request",
            "identifying_labels":["implementation"]
          }],
          "queues":[{
            "id":"pr_ci_recovery",
            "artifact":"implementation_pr",
            "condition":{"kind":"ci_recovery_required"},
            "actions":[{
              "role":"ci_diagnostician",
              "action":"diagnose_interrupted_ci",
              "checkout":"pull_request_read_only"
            }]
          }],
          "transitions":[
            {
              "id":"diagnose_interrupted_ci",
              "artifact":"implementation_pr",
              "roles":["ci_diagnostician"],
              "outcomes":{"diagnosed":"record_interrupted_ci_diagnostic"},
              "effects":[]
            },
            {
              "id":"record_interrupted_ci_diagnostic",
              "artifact":"implementation_pr",
              "roles":["ci_diagnostician"],
              "effects":[]
            }
          ]
        }"#,
    )
    .unwrap()
    .validate()
    .unwrap()
}

fn workflow_without_diagnostic() -> ValidatedWorkflow {
    serde_json::from_str::<RawWorkflowSpec>(
        r#"{
          "name":"interrupted-recovery-without-diagnostic",
          "labels":[
            {"id":"implementation"},
            {"id":"needs-human"}
          ],
          "artifact_kinds":[{
            "id":"implementation_pr",
            "target":"pull_request",
            "identifying_labels":["implementation"]
          }]
        }"#,
    )
    .unwrap()
    .validate()
    .unwrap()
}

fn job(
    repository: &Repository,
    pull_request: &PullRequest,
    run: &str,
    attempt: &str,
    status: CiJobStatus,
    conclusion: Option<CiJobConclusion>,
    now: DateTime<Utc>,
) -> CiJob {
    CiJob {
        id: CiJobId::new(format!("job-{run}-{attempt}")),
        repo_id: repository.id.clone(),
        pull_request_id: Some(pull_request.id.clone()),
        commit_sha: pull_request.head_sha.clone().unwrap(),
        name: "validate".into(),
        status,
        conclusion,
        provider_conclusion: conclusion.map(|value| format!("{value:?}").to_lowercase()),
        provider_reason: Some("runner process disappeared after host restart".into()),
        run_id: Some(run.into()),
        attempt: Some(attempt.into()),
        verified_failure: None,
        url: Some(format!("https://forge.example/actions/runs/{run}")),
        created_at: now,
        started_at: Some(now - Duration::minutes(15)),
        completed_at: (status == CiJobStatus::Completed).then_some(now),
        updated_at: now,
    }
}

fn diagnostic_assignment(fixture: &Fixture) -> DurableAssignment {
    DurableAssignment {
        job_id: Some(format!(
            "ai/temper/pull_request-{}/ci_diagnostician/pr_ci_recovery",
            fixture.pull_request.number.get()
        )),
        attempt_id: Some("diagnostic-attempt-1".into()),
        role: Some(RoleId::new("ci_diagnostician")),
        queue: Some("pr_ci_recovery".into()),
        action: Some("diagnose_interrupted_ci".into()),
        worker_id: Some("diagnostic-worker".into()),
        daemon_boot_id: Some("boot-1".into()),
        ..DurableAssignment::default()
    }
}

async fn complete_diagnostic(fixture: &Fixture) {
    assert_eq!(
        fixture.recover().await,
        InterruptedCiRecoveryOutcome::Waiting
    );
    assert_eq!(
        fixture.recover().await,
        InterruptedCiRecoveryOutcome::DispatchDiagnostic
    );
    let manager = LeaseManager::new(&fixture.forge, LeasePolicy::new(Duration::minutes(30)));
    let expected = diagnostic_assignment(fixture);
    let claimed = manager
        .claim_assignment(
            &fixture.repository.id,
            ArtifactSource::PullRequest {
                number: fixture.pull_request.number,
            },
            AssignmentClaimRequest {
                assignment: expected,
                mutation: AssignmentMutation::default(),
            },
            fixture.now,
        )
        .await
        .unwrap();
    manager
        .release_assignment(
            &fixture.repository.id,
            ArtifactSource::PullRequest {
                number: fixture.pull_request.number,
            },
            &claimed,
        )
        .await
        .unwrap();
}

#[test]
fn unsupported_retry_dispatches_one_diagnostic_then_parks_with_one_audit() {
    temper_engine_io::block_on(async {
        let fixture = Fixture::new().await;
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Waiting
        );
        assert_eq!(fixture.forge.ci_retry_requests().len(), 1);
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::DispatchDiagnostic
        );
        let item = temper_runner::WorkItem {
            queue: temper_workflow::QueueId::new("pr_ci_recovery"),
            role: RoleId::new("ci_diagnostician"),
            target: ArtifactSource::PullRequest {
                number: fixture.pull_request.number,
            },
            kind: temper_workflow::ArtifactKindId::new("implementation_pr"),
        };
        let mut job = crate::feed::job_from_work_item("ai/temper", &item);
        assert_eq!(
            crate::feed::enrich_work_item_job(
                &fixture.forge,
                &fixture.repository.id,
                &item,
                &mut job,
                &fixture.workflow,
                &fixture.compiled,
            )
            .await
            .unwrap(),
            crate::feed::EnrichOutcome::Enriched
        );
        let context: temper_protocol_worker::JobContext =
            serde_json::from_value(job.job_payload).unwrap();
        assert_eq!(
            context.checkout_capability.as_deref(),
            Some("pull_request_read_only")
        );
        assert_eq!(context.allowed_verdicts, vec!["diagnosed"]);
        assert!(
            context
                .workspace
                .unwrap()
                .repos
                .iter()
                .all(|repo| repo.access == temper_protocol_worker::RepoAccess::ReadOnly)
        );
        let guidance = context.guidance.unwrap();
        assert!(guidance.contains("READ-ONLY"));
        assert!(!guidance.contains("Do NOT report success without changing files"));

        let manager = LeaseManager::new(&fixture.forge, LeasePolicy::new(Duration::minutes(30)));
        let expected = diagnostic_assignment(&fixture);
        let claimed = manager
            .claim_assignment(
                &fixture.repository.id,
                ArtifactSource::PullRequest {
                    number: fixture.pull_request.number,
                },
                AssignmentClaimRequest {
                    assignment: expected.clone(),
                    mutation: AssignmentMutation::default(),
                },
                fixture.now,
            )
            .await
            .unwrap();
        let state = parse_metadata_block(&fixture.fresh().await.body)
            .unwrap()
            .unwrap()
            .interrupted_ci_recovery
            .unwrap();
        assert_eq!(
            state.diagnostic.unwrap().job_id,
            expected.job_id,
            "diagnostic publication boundary is committed with assignment claim"
        );
        manager
            .release_assignment(
                &fixture.repository.id,
                ArtifactSource::PullRequest {
                    number: fixture.pull_request.number,
                },
                &claimed,
            )
            .await
            .unwrap();

        fixture
            .forge
            .fail_next(FaultOp::AddPullRequestComment, "audit response interrupted");
        assert!(matches!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Retryable { .. }
        ));
        let interrupted = fixture.fresh().await;
        assert!(requires_human_attention(&interrupted.labels));
        assert!(
            parse_metadata_block(&interrupted.body)
                .unwrap()
                .unwrap()
                .interrupted_ci_recovery
                .is_some(),
            "audit failure retains the durable cleanup marker"
        );
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Parked
        );
        let parked = fixture.fresh().await;
        assert!(requires_human_attention(&parked.labels));
        assert!(
            parse_metadata_block(&parked.body)
                .unwrap()
                .unwrap()
                .interrupted_ci_recovery
                .is_none()
        );
        let comments = fixture
            .forge
            .list_pull_request_comments(&parked.id)
            .await
            .unwrap();
        assert_eq!(comments.len(), 1);
        assert!(comments[0].body.contains("runner process disappeared"));
        assert!(comments[0].body.contains("Run: `591`"));
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Suppressed
        );
        assert_eq!(
            fixture
                .forge
                .list_pull_request_comments(&parked.id)
                .await
                .unwrap()
                .len(),
            1
        );
    });
}

#[test]
fn uncertain_retry_is_never_repeated_and_falls_back_to_diagnostic() {
    temper_engine_io::block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .forge
            .fail_next(FaultOp::RetryCiAttempt, "response lost");
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Waiting
        );
        assert!(fixture.forge.ci_retry_requests().is_empty());
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::DispatchDiagnostic
        );
        assert!(fixture.forge.ci_retry_requests().is_empty());
        let state = parse_metadata_block(&fixture.fresh().await.body)
            .unwrap()
            .unwrap()
            .interrupted_ci_recovery
            .unwrap();
        assert_eq!(state.retry_outcome, Some(CiRetryOutcome::Uncertain));
    });
}

#[test]
fn accepted_retry_converges_on_newer_pending_attempt_without_diagnostic() {
    temper_engine_io::block_on(async {
        let fixture = Fixture::new().await;
        fixture.forge.set_ci_retry_outcome(CiRetryOutcome::Accepted);
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Waiting
        );
        assert_eq!(fixture.forge.ci_retry_requests().len(), 1);

        fixture.forge.seed_ci_jobs(
            &fixture.repository.id,
            vec![job(
                &fixture.repository,
                &fixture.pull_request,
                "591",
                "2",
                CiJobStatus::Running,
                None,
                fixture.now + Duration::minutes(1),
            )],
        );
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Suppressed
        );
        assert!(
            parse_metadata_block(&fixture.fresh().await.body)
                .unwrap()
                .unwrap()
                .interrupted_ci_recovery
                .is_none()
        );
        assert_eq!(fixture.forge.ci_retry_requests().len(), 1);
    });
}

#[test]
fn changed_head_suppresses_stale_recovery_without_side_effects() {
    temper_engine_io::block_on(async {
        let fixture = Fixture::new().await;
        fixture.forge.set_ci_retry_outcome(CiRetryOutcome::Accepted);
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Waiting
        );
        fixture
            .forge
            .set_pull_request_head(&fixture.pull_request.id, Some("new-head-1234567".into()))
            .unwrap();
        assert_eq!(
            fixture.recover().await,
            InterruptedCiRecoveryOutcome::Suppressed
        );
        assert!(!requires_human_attention(&fixture.fresh().await.labels));
        assert_eq!(fixture.forge.ci_retry_requests().len(), 1);
    });
}

#[path = "interrupted_ci_recovery_tests/faults.rs"]
mod faults;
