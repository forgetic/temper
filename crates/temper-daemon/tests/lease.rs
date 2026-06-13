// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use serde_json::json;
use temper_daemon::{LeaseApplier, ResultApplier};
use temper_forge::{CreateIssue, CreateRepository, Forge, ItemNumber, RepositoryId, UserId};
use temper_forge_memory::MemoryForge;
use temper_worker_protocol::{
    Artifact, Branch, JobResult, RepoOutcome, ResultStatus, WORKER_PROTOCOL_VERSION,
};
use temper_worker_registry::InFlightJob;
use temper_workflow::{parse_metadata_block, ArtifactSource, LeaseManager, LeasePolicy, RoleId};

struct RecordingApplier {
    tx: temper_io_engine::CqSender<(InFlightJob, JobResult)>,
    forge: Option<Arc<MemoryForge>>,
    repo: Option<RepositoryId>,
    issue: Option<ItemNumber>,
    lease_tx: Option<temper_io_engine::CqSender<Option<temper_workflow::Lease>>>,
}

#[async_trait::async_trait]
impl ResultApplier for RecordingApplier {
    async fn apply(&self, job: InFlightJob, result: JobResult) {
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
        role: "engineer".to_string(),
        repo: "acme/service".to_string(),
        artifact: Artifact {
            item: json!(number.get()),
            kind: "issue".to_string(),
        },
        job_payload: json!({}),
    }
}

fn job_result(job_id: &str) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-a".to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Success,
        repos: vec![RepoOutcome {
            repo: "acme/service".to_string(),
            branch: Branch {
                name: "agent/pr-for-code-118".to_string(),
                head_sha: "abc123".to_string(),
            },
        }],
        verdict: None,
        body: None,
        children: Vec::new(),
        failure: None,
        summary: Some("done".to_string()),
        details: Some(json!({"note":"fake worker result"})),
    }
}

#[test]
fn lease_won_inner_applied_then_lease_released() {
    temper_io_engine::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let (tx, mut rx) = temper_io_engine::channel();
        let (lease_tx, mut lease_rx) = temper_io_engine::channel();
        let inner = Arc::new(RecordingApplier {
            tx,
            forge: Some(forge.clone()),
            repo: Some(repo.clone()),
            issue: Some(issue),
            lease_tx: Some(lease_tx),
        });
        let applier = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            inner,
            temper_daemon::system_clock(),
        );
        let job = in_flight_job(issue);
        let result = job_result(&job.job_id);

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
        assert!(matches!(lease_rx.try_recv(), None));

        let issue = forge
            .get_issue_by_number(&repo, issue)
            .await
            .expect("issue reload succeeds")
            .expect("issue exists after apply");
        assert!(parse_metadata_block(&issue.body)
            .expect("issue metadata parses")
            .unwrap_or_default()
            .lease
            .is_none());
    })
}

#[test]
fn peer_owned_lease_noops_and_preserves_peer_lease() {
    temper_io_engine::block_on_with(move |_cx, _handle| async move {
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

        let (tx, mut rx) = temper_io_engine::channel();
        let inner = Arc::new(RecordingApplier {
            tx,
            forge: None,
            repo: None,
            issue: None,
            lease_tx: None,
        });
        let applier = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            inner,
            temper_daemon::system_clock(),
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
