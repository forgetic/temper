// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::json;
use temper_engine::{ApplyOutcome, ClaimContext, ClaimOutcome, Daemon, InFlightJob, ResultApplier};
use temper_protocol_worker::{
    Artifact, Capability, Capacity, ErrorCode, JobResult, Poll, Register, WORKER_PROTOCOL_VERSION,
    WorkerProtocolMessage,
};

struct FailFirstClaim {
    failed: AtomicBool,
}

#[async_trait::async_trait]
impl ResultApplier for FailFirstClaim {
    async fn claim(&self, _job: InFlightJob, _context: ClaimContext) -> ClaimOutcome {
        if !self.failed.swap(true, Ordering::SeqCst) {
            ClaimOutcome::Retryable {
                reason: "simulated Forge outage".to_string(),
            }
        } else {
            ClaimOutcome::Claimed
        }
    }

    async fn apply(&self, _job: InFlightJob, _result: JobResult) -> ApplyOutcome {
        ApplyOutcome::Applied
    }
}

fn register() -> WorkerProtocolMessage {
    WorkerProtocolMessage::Register(Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-a".to_string(),
        capabilities: vec![Capability {
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
        }],
        capacity: Capacity {
            max_concurrent_jobs: 1,
        },
        worker_pool: None,
        labels: None,
    })
}

fn poll() -> WorkerProtocolMessage {
    WorkerProtocolMessage::Poll(Poll {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: "worker-a".to_string(),
        free_capacity: 1,
        max_wait_ms: Some(0),
    })
}

#[test]
fn failed_durable_claim_is_rolled_back_and_redispatchable() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let daemon = Daemon::with_applier(
            Arc::new(handle),
            Arc::new(FailFirstClaim {
                failed: AtomicBool::new(false),
            }),
        );
        assert_eq!(
            daemon.deliver_protocol_message(register()).await.unwrap(),
            None
        );
        daemon
            .enqueue_job(
                "job-claim",
                "engineer",
                "ai/temper",
                Artifact {
                    item: json!(257),
                    kind: "issue".to_string(),
                },
                json!({}),
            )
            .await;

        match daemon.deliver_protocol_message(poll()).await.unwrap() {
            Some(WorkerProtocolMessage::Error(error)) => {
                assert_eq!(error.code, ErrorCode::PollTimeout)
            }
            other => panic!("expected failed claim, got {other:?}"),
        }
        match daemon.deliver_protocol_message(poll()).await.unwrap() {
            Some(WorkerProtocolMessage::Assign(assign)) => assert_eq!(assign.job_id, "job-claim"),
            other => panic!("expected redispatched assignment, got {other:?}"),
        }
    })
}
