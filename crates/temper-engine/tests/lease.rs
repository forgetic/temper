// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use serde_json::json;
use temper_engine::{LeaseApplier, ResultApplier};
use temper_forge::{
    BranchRef, CreateIssue, CreatePullRequest, CreateRepository, Forge, ItemNumber, RepositoryId,
    UserId,
};
use temper_forge_memory::MemoryForge;
use temper_protocol_worker::{
    Artifact, Branch, JobResult, PullRequestFreshness, PullRequestFreshnessResponse, RepoOutcome,
    ResultStatus, WORKER_PROTOCOL_VERSION,
};
use temper_worker_registry::InFlightJob;
use temper_workflow::{ArtifactSource, LeaseManager, LeasePolicy, RoleId, parse_metadata_block};

struct RecordingApplier {
    tx: temper_engine_io::CqSender<(InFlightJob, JobResult)>,
    forge: Option<Arc<MemoryForge>>,
    repo: Option<RepositoryId>,
    issue: Option<ItemNumber>,
    lease_tx: Option<temper_engine_io::CqSender<Option<temper_workflow::Lease>>>,
    freshness_response: Option<PullRequestFreshnessResponse>,
}

#[async_trait::async_trait]
impl ResultApplier for RecordingApplier {
    async fn apply(&self, job: InFlightJob, result: JobResult) -> temper_engine::ApplyOutcome {
        if let (Some(forge), Some(repo), Some(issue), Some(lease_tx)) =
            (&self.forge, &self.repo, self.issue, &self.lease_tx)
        {
            let issue = forge
                .get_issue_by_number(repo, issue)
                .await
                .expect("issue reload succeeds")
                .expect("issue exists while applying");
            let lease = parse_metadata_block(&issue.body)
                .expect("issue metadata parses")
                .unwrap_or_default()
                .lease;
            let _ = lease_tx.send(lease);
        }

        let _ = self.tx.send((job, result));
        temper_engine::ApplyOutcome::Applied
    }

    async fn check_pull_request_freshness(
        &self,
        _check: PullRequestFreshness,
    ) -> PullRequestFreshnessResponse {
        self.freshness_response.clone().unwrap_or_else(|| {
            PullRequestFreshnessResponse::unavailable("freshness response not configured")
        })
    }
}

async fn new_repo(forge: &MemoryForge) -> RepositoryId {
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

async fn create_ready_issue(forge: &MemoryForge, repo: &RepositoryId) -> ItemNumber {
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: "ready code issue".to_string(),
                body: "Implement the feature.".to_string(),
                labels: vec!["code".to_string(), "ready".to_string()],
                assignees: Vec::<UserId>::new(),
            },
        )
        .await
        .expect("issue is created")
        .number
}

fn policy() -> LeasePolicy {
    LeasePolicy::new(chrono::Duration::seconds(300))
}

fn in_flight_job(number: ItemNumber) -> InFlightJob {
    InFlightJob {
        job_id: "ai/test-job".to_string(),
        attempt_id: Some("attempt-test".to_string()),
        role: "engineer".to_string(),
        repo: "acme/service".to_string(),
        artifact: Artifact {
            item: json!(number.get()),
            kind: "issue".to_string(),
        },
        job_payload: json!({}),
    }
}

async fn create_repair_pull_request(
    forge: &MemoryForge,
    repo: &RepositoryId,
    branch: &str,
) -> temper_forge::PullRequest {
    let pull_request = forge
        .create_pull_request(
            repo,
            CreatePullRequest {
                title: "repair conflicted PR".to_string(),
                body: "repair this PR".to_string(),
                source: BranchRef {
                    repository_id: repo.clone(),
                    branch: branch.to_string(),
                },
                target: BranchRef {
                    repository_id: repo.clone(),
                    branch: "main".to_string(),
                },
                labels: vec!["implementation".to_string(), "merge-conflict".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("pull request is created");
    forge
        .set_pull_request_head(&pull_request.id, Some("assigned-head".to_string()))
        .expect("pull request head is set")
}

fn repair_job(repo: &RepositoryId, pull_request: &temper_forge::PullRequest) -> InFlightJob {
    InFlightJob {
        job_id: format!(
            "acme/service/pull_request-{}/engineer/pr_merge_conflict",
            pull_request.number
        ),
        attempt_id: Some("attempt-repair".to_string()),
        role: "engineer".to_string(),
        repo: "acme/service".to_string(),
        artifact: Artifact {
            item: json!(pull_request.number.get()),
            kind: "pull_request".to_string(),
        },
        job_payload: json!({
            "role": "engineer",
            "repo": "acme/service",
            "queue": "pr_merge_conflict",
            "artifact_kind": "implementation_pr",
            "action": "resolve_merge_conflict",
            "checkout_capability": "pull_request_writable",
            "pull_request_freshness": {
                "repository_id": repo.as_str(),
                "repo": "acme/service",
                "role": "engineer",
                "queue": "pr_merge_conflict",
                "action": "resolve_merge_conflict",
                "number": pull_request.number.get(),
                "pull_request_id": pull_request.id.as_str(),
                "head_sha": "assigned-head",
                "queue_labels": ["merge-conflict"]
            }
        }),
    }
}

fn job_result(job_id: &str) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-a".to_string(),
        job_id: job_id.to_string(),
        attempt_id: Some(if job_id == "ai/test-job" {
            "attempt-test".to_string()
        } else {
            "attempt-repair".to_string()
        }),
        status: ResultStatus::Success,
        repos: vec![RepoOutcome {
            repo: "acme/service".to_string(),
            branch: Branch {
                name: "agent/pr-for-code-118".to_string(),
                head_sha: "abc123".to_string(),
            },
        }],
        verdict: None,
        title: None,
        body: None,
        children: Vec::new(),
        failure: None,
        summary: Some("done".to_string()),
        details: Some(json!({"note":"fake worker result"})),
    }
}

#[path = "lease/lookup_failures.rs"]
mod lookup_failures;
#[path = "lease/recovered_ownership.rs"]
mod recovered_ownership;

#[test]
fn claim_revalidates_pr_freshness_before_persisting_assignment() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let cases = [
            (
                PullRequestFreshnessResponse::stale("merge-conflict label was removed"),
                temper_engine::ClaimOutcome::Stale {
                    reason: "merge-conflict label was removed".to_string(),
                },
            ),
            (
                PullRequestFreshnessResponse::unavailable("Forge read timed out"),
                temper_engine::ClaimOutcome::Retryable {
                    reason: "Forge read timed out".to_string(),
                },
            ),
        ];

        for (index, (freshness_response, expected)) in cases.into_iter().enumerate() {
            let pull_request =
                create_repair_pull_request(&forge, &repo, &format!("agent/repair-{index}")).await;
            let (tx, _rx) = temper_engine_io::channel();
            let inner = Arc::new(RecordingApplier {
                tx,
                forge: None,
                repo: None,
                issue: None,
                lease_tx: None,
                freshness_response: Some(freshness_response),
            });
            let applier = LeaseApplier::new(
                forge.clone(),
                policy(),
                "daemon-1",
                inner,
                temper_engine::system_clock(),
            );
            let job = repair_job(&repo, &pull_request);

            assert_eq!(
                applier
                    .claim(
                        job,
                        temper_engine::ClaimContext {
                            worker_id: "worker-a".to_string(),
                            daemon_boot_id: "daemon-1".to_string(),
                        },
                    )
                    .await,
                expected
            );

            let current = forge
                .get_pull_request_by_number(&repo, pull_request.number)
                .await
                .expect("pull request reload succeeds")
                .expect("pull request still exists");
            let metadata = parse_metadata_block(&current.body)
                .expect("pull request metadata parses")
                .unwrap_or_default();
            assert!(metadata.assignment.is_none());
            assert!(metadata.lease.is_none());
        }
    })
}

#[test]
fn lease_won_inner_applied_then_lease_released() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let (tx, mut rx) = temper_engine_io::channel();
        let (lease_tx, mut lease_rx) = temper_engine_io::channel();
        let inner = Arc::new(RecordingApplier {
            tx,
            forge: Some(forge.clone()),
            repo: Some(repo.clone()),
            issue: Some(issue),
            lease_tx: Some(lease_tx),
            freshness_response: None,
        });
        let applier = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            inner,
            temper_engine::system_clock(),
        );
        let job = in_flight_job(issue);
        let result = job_result(&job.job_id);
        assert_eq!(
            applier
                .claim(
                    job.clone(),
                    temper_engine::ClaimContext {
                        worker_id: result.worker_id.clone(),
                        daemon_boot_id: "daemon-1".to_string(),
                    },
                )
                .await,
            temper_engine::ClaimOutcome::Claimed
        );

        applier.apply(job.clone(), result.clone()).await;

        let (recorded_job, recorded_result) = rx.recv().await.expect("inner records one apply");
        assert_eq!(recorded_job, job);
        assert_eq!(recorded_result, result);
        assert!(rx.try_recv().is_none());

        let observed_lease = lease_rx
            .recv()
            .await
            .expect("inner records lease state")
            .expect("lease is present while inner apply runs");
        assert_eq!(observed_lease.worker, "daemon-1");
        assert_eq!(observed_lease.role, RoleId::new("engineer"));
        assert!(lease_rx.try_recv().is_none());

        let issue = forge
            .get_issue_by_number(&repo, issue)
            .await
            .expect("issue reload succeeds")
            .expect("issue exists after apply");
        assert!(
            parse_metadata_block(&issue.body)
                .expect("issue metadata parses")
                .unwrap_or_default()
                .lease
                .is_none()
        );
    })
}

#[test]
fn matching_recovered_result_reattaches_and_releases_durable_assignment() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let job = in_flight_job(issue);
        let result = job_result(&job.job_id);
        let context = temper_engine::ClaimContext {
            worker_id: result.worker_id.clone(),
            daemon_boot_id: "daemon-boot-original".to_string(),
        };

        let (initial_tx, _initial_rx) = temper_engine_io::channel();
        let initial = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            Arc::new(RecordingApplier {
                tx: initial_tx,
                forge: None,
                repo: None,
                issue: None,
                lease_tx: None,
                freshness_response: None,
            }),
            temper_engine::system_clock(),
        );
        assert_eq!(
            initial.claim(job.clone(), context.clone()).await,
            temper_engine::ClaimOutcome::Claimed
        );
        drop(initial);

        let (tx, mut rx) = temper_engine_io::channel();
        let recovered = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            Arc::new(RecordingApplier {
                tx,
                forge: None,
                repo: None,
                issue: None,
                lease_tx: None,
                freshness_response: None,
            }),
            temper_engine::system_clock(),
        );
        assert_eq!(
            recovered
                .apply_recovered(job.clone(), result.clone(), context)
                .await,
            temper_engine::ApplyOutcome::Applied
        );
        assert_eq!(rx.recv().await, Some((job, result)));

        let issue = forge
            .get_issue_by_number(&repo, issue)
            .await
            .expect("issue reload succeeds")
            .expect("issue exists after recovered apply");
        let metadata = parse_metadata_block(&issue.body)
            .expect("issue metadata parses")
            .unwrap_or_default();
        assert!(metadata.assignment.is_none());
        assert!(metadata.lease.is_none());
    })
}

#[test]
fn peer_owned_lease_noops_and_preserves_peer_lease() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let target = ArtifactSource::Issue { number: issue };
        let manager = LeaseManager::new(forge.as_ref(), policy());
        let peer_lease = manager
            .acquire(
                &repo,
                target,
                RoleId::new("engineer"),
                "daemon-2",
                chrono::Utc::now(),
            )
            .await
            .expect("peer lease is acquired");

        let (tx, mut rx) = temper_engine_io::channel();
        let inner = Arc::new(RecordingApplier {
            tx,
            forge: None,
            repo: None,
            issue: None,
            lease_tx: None,
            freshness_response: None,
        });
        let applier = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            inner,
            temper_engine::system_clock(),
        );
        let job = in_flight_job(issue);
        let result = job_result(&job.job_id);

        applier.apply(job, result).await;

        assert!(rx.try_recv().is_none());
        let issue = forge
            .get_issue_by_number(&repo, issue)
            .await
            .expect("issue reload succeeds")
            .expect("issue exists after duplicate apply");
        let lease = parse_metadata_block(&issue.body)
            .expect("issue metadata parses")
            .expect("issue has metadata")
            .lease
            .expect("peer lease is still present");
        assert_eq!(lease, peer_lease);
    })
}
