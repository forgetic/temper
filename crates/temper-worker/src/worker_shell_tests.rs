use temper_protocol_worker::{AttemptCancellation, CancelAttempts, WorkerProtocolMessage};

use super::{cancel_job_control, heartbeat_outcome};
use crate::executor::{AttemptFence, JobCancellation, JobCancellationRequest};
use crate::task_registry::{ActiveJobJoinState, ActiveJobTask, WorkerTaskRegistry};
use crate::worker_machine::WorkerCompletion;

#[test]
fn heartbeat_outcome_preserves_parsed_cancellation_directives() {
    let directive = CancelAttempts::new(
        "worker-1",
        vec![
            AttemptCancellation::ownership_lost(
                "worker-1",
                "job-1",
                "attempt-1",
                "durable claim removed",
            )
            .unwrap(),
        ],
    )
    .unwrap();

    assert_eq!(
        heartbeat_outcome(Ok(Some(WorkerProtocolMessage::CancelAttempts(
            directive.clone(),
        )))),
        Ok(Some(directive))
    );
    assert_eq!(heartbeat_outcome(Ok(None)), Ok(None));
}

#[test]
fn cancel_job_control_closes_the_exact_attempt_fence_before_returning() {
    let registry = WorkerTaskRegistry::new();
    let fence = AttemptFence::open();
    let cancellation = JobCancellation::default();
    let task = ActiveJobTask::new("job-1", "attempt-1", 7, fence.clone(), cancellation.clone());
    assert!(registry.register(task));
    let (cq, _rx) = temper_worker_io::channel::<WorkerCompletion>();

    cancel_job_control(
        registry.clone(),
        cq,
        "job-1".to_string(),
        "attempt-1".to_string(),
        7,
    );

    assert!(!fence.is_open());
    assert_eq!(
        cancellation.requested(),
        Some(JobCancellationRequest::Graceful)
    );
    assert_eq!(
        registry.active_jobs()[0].join_state(),
        ActiveJobJoinState::CancellationRequested
    );
}
