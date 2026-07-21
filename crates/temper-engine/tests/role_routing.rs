// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use serde_json::json;
use temper_engine::{
    RecoveredHeartbeatOutcome, RecoveredOwnershipLossReason, ResultApplier, RoleRoutingApplier,
};
use temper_protocol_worker::{Artifact, JobResult, ResultStatus, WORKER_PROTOCOL_VERSION};
use temper_worker_registry::InFlightJob;

struct RecordingApplier {
    name: &'static str,
    tx: temper_engine_io::CqSender<(&'static str, String, String)>,
}

impl RecordingApplier {
    fn new(
        name: &'static str,
        tx: temper_engine_io::CqSender<(&'static str, String, String)>,
    ) -> Self {
        Self { name, tx }
    }
}

#[async_trait::async_trait]
impl ResultApplier for RecordingApplier {
    async fn apply(&self, job: InFlightJob, result: JobResult) -> temper_engine::ApplyOutcome {
        self.tx
            .send((self.name, job.role, result.job_id))
            .expect("recording receiver is open");
        temper_engine::ApplyOutcome::Applied
    }
}

struct HeartbeatApplier {
    outcome: RecoveredHeartbeatOutcome,
}

#[async_trait::async_trait]
impl ResultApplier for HeartbeatApplier {
    async fn heartbeat(
        &self,
        _job: InFlightJob,
        _context: temper_engine::ClaimContext,
    ) -> RecoveredHeartbeatOutcome {
        self.outcome.clone()
    }

    async fn apply(&self, _job: InFlightJob, _result: JobResult) -> temper_engine::ApplyOutcome {
        temper_engine::ApplyOutcome::Applied
    }
}

fn claim_context() -> temper_engine::ClaimContext {
    temper_engine::ClaimContext {
        worker_id: "worker-a".to_string(),
        daemon_boot_id: "boot-a".to_string(),
    }
}

fn in_flight_job(role: &str) -> InFlightJob {
    InFlightJob {
        job_id: format!("job-{role}"),
        attempt_id: Some(format!("attempt-{role}")),
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
        attempt_id: Some(job_id.replacen("job-", "attempt-", 1)),
        status: ResultStatus::Success,
        repos: Vec::new(),
        verdict: None,
        title: None,
        body: None,
        children: Vec::new(),
        failure: None,
        summary: None,
        details: None,
    }
}

#[test]
fn role_routing_preserves_typed_heartbeat_outcomes_for_routes_and_default() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let transient = RecoveredHeartbeatOutcome::TransientlyUnavailable {
            reason: "Forge unavailable".to_string(),
        };
        let lost = RecoveredHeartbeatOutcome::OwnershipLost {
            reason: RecoveredOwnershipLossReason::LeaseReplaced,
        };
        let routing = RoleRoutingApplier::new(Arc::new(HeartbeatApplier {
            outcome: transient.clone(),
        }))
        .with_route(
            "engineer",
            Arc::new(HeartbeatApplier {
                outcome: lost.clone(),
            }),
        );

        assert_eq!(
            routing
                .heartbeat(in_flight_job("engineer"), claim_context())
                .await,
            lost
        );
        assert_eq!(
            routing
                .heartbeat(in_flight_job("reviewer"), claim_context())
                .await,
            transient
        );
        assert_eq!(
            temper_engine::NoopApplier
                .heartbeat(in_flight_job("architect"), claim_context())
                .await,
            RecoveredHeartbeatOutcome::Owned
        );
    })
}

#[test]
fn role_routing_applier_dispatches_known_role_to_registered_applier() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let (tx, mut rx) = temper_engine_io::channel();
        let routing =
            RoleRoutingApplier::new(Arc::new(RecordingApplier::new("default", tx.clone())))
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
        assert!(rx.try_recv().is_none());
    })
}

#[test]
fn role_routing_applier_dispatches_unknown_role_to_default_applier() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let (tx, mut rx) = temper_engine_io::channel();
        let routing =
            RoleRoutingApplier::new(Arc::new(RecordingApplier::new("default", tx.clone())))
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
        assert!(rx.try_recv().is_none());
    })
}
