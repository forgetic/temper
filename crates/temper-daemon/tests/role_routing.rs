// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use serde_json::json;
use temper_daemon::{ResultApplier, RoleRoutingApplier};
use temper_worker_protocol::{Artifact, JobResult, ResultStatus, WORKER_PROTOCOL_VERSION};
use temper_worker_registry::InFlightJob;
use tokio::sync::mpsc;

struct RecordingApplier {
    name: &'static str,
    tx: mpsc::UnboundedSender<(&'static str, String, String)>,
}

impl RecordingApplier {
    fn new(name: &'static str, tx: mpsc::UnboundedSender<(&'static str, String, String)>) -> Self {
        Self { name, tx }
    }
}

#[async_trait::async_trait]
impl ResultApplier for RecordingApplier {
    async fn apply(&self, job: InFlightJob, result: JobResult) {
        self.tx
            .send((self.name, job.role, result.job_id))
            .expect("recording receiver is open");
    }
}

fn in_flight_job(role: &str) -> InFlightJob {
    InFlightJob {
        job_id: format!("job-{role}"),
        role: role.to_string(),
        repo: "ai/temper".to_string(),
        artifact: Artifact {
            item: json!(1),
            kind: "issue".to_string(),
        },
        job_payload: json!({}),
    }
}

fn success_result(job_id: &str) -> JobResult {
    JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-a".to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Success,
        branch: None,
        failure: None,
        summary: None,
        details: None,
    }
}

#[tokio::test]
async fn role_routing_applier_dispatches_known_role_to_registered_applier() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let routing = RoleRoutingApplier::new(Arc::new(RecordingApplier::new("default", tx.clone())))
        .with_route("engineer", Arc::new(RecordingApplier::new("engineer", tx)));
    let job = in_flight_job("engineer");

    routing
        .apply(job.clone(), success_result(&job.job_id))
        .await;

    assert_eq!(
        rx.recv().await,
        Some((
            "engineer",
            "engineer".to_string(),
            "job-engineer".to_string()
        ))
    );
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn role_routing_applier_dispatches_unknown_role_to_default_applier() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let routing = RoleRoutingApplier::new(Arc::new(RecordingApplier::new("default", tx.clone())))
        .with_route("engineer", Arc::new(RecordingApplier::new("engineer", tx)));
    let job = in_flight_job("reviewer");

    routing
        .apply(job.clone(), success_result(&job.job_id))
        .await;

    assert_eq!(
        rx.recv().await,
        Some((
            "default",
            "reviewer".to_string(),
            "job-reviewer".to_string()
        ))
    );
    assert!(rx.try_recv().is_err());
}
