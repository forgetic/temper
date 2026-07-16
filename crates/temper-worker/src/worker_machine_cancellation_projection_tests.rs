//! Projection tests for joined cancellation reports.

use std::time::Duration;

use temper_protocol_worker::WorkerProtocolMessage;
use temper_worker_io::{EngineTime, Machine};

use super::tests::{assign, params};
use super::{JobCleanup, WatchdogTimerKind, WorkerCompletion, WorkerMachine, WorkerRequest};
use crate::executor::{CancellationOutcome, DescendantCleanupStatus};

fn dispatch_at(machine: &mut WorkerMachine, job_id: &str, now: EngineTime) -> Vec<WorkerRequest> {
    machine.on_completion(
        now,
        WorkerCompletion::PollReply(Ok(Some(WorkerProtocolMessage::Assign(assign(job_id))))),
    )
}

#[test]
fn real_cancellation_and_cleanup_outcomes_project_without_synthesis() {
    let cases = [
        (
            CancellationOutcome::Graceful,
            DescendantCleanupStatus::Clean,
            "graceful",
            "clean",
            false,
            None,
        ),
        (
            CancellationOutcome::ForcedTermination,
            DescendantCleanupStatus::Terminated,
            "forced_termination",
            "terminated",
            true,
            None,
        ),
        (
            CancellationOutcome::HardKill,
            DescendantCleanupStatus::HardKilled,
            "hard_kill",
            "hard_killed",
            true,
            None,
        ),
        (
            CancellationOutcome::HardKill,
            DescendantCleanupStatus::Failed("injected containment failure".to_string()),
            "hard_kill",
            "failed",
            true,
            Some("injected containment failure"),
        ),
    ];

    for (
        index,
        (cancellation, descendants, expected_outcome, expected_descendants, forced, error),
    ) in cases.into_iter().enumerate()
    {
        let job_id = format!("job-projection-{index}");
        let attempt_id = format!("attempt-{job_id}");
        let mut config = params();
        config.liveness_limits.max_no_progress = Duration::from_nanos(10);
        let mut machine = WorkerMachine::new(config);
        dispatch_at(&mut machine, &job_id, EngineTime::ZERO);
        let state = machine.job_state(&job_id).unwrap();
        let generation = state.generation;
        let timer_generation = state.timer_generation;
        machine.on_completion(
            EngineTime::from_nanos(11),
            WorkerCompletion::WatchdogTimer {
                job_id: job_id.clone(),
                attempt_id: attempt_id.clone(),
                generation,
                timer_generation,
                kind: WatchdogTimerKind::NoProgress,
            },
        );

        let requests = machine.on_completion(
            EngineTime::from_nanos(12),
            WorkerCompletion::JobQuiesced {
                job_id: job_id.clone(),
                attempt_id,
                generation,
                cleanup: JobCleanup {
                    cancellation,
                    descendants,
                },
            },
        );
        assert!(requests.iter().any(|request| matches!(
            request,
            WorkerRequest::Observe(crate::observability::WorkerEvent::CancellationCompleted {
                outcome,
                descendant_cleanup,
                forced: observed_forced,
                ..
            }) if outcome == expected_outcome
                && descendant_cleanup == expected_descendants
                && *observed_forced == forced
        )));
        let result = requests
            .iter()
            .find_map(|request| match request {
                WorkerRequest::RecordResult { result, .. } => Some(result),
                _ => None,
            })
            .expect("real cancellation outcome creates timeout result");
        let cleanup = &result.details.as_ref().unwrap()["timeout"]["cleanup"];
        assert_eq!(cleanup["cancellation"], expected_outcome);
        assert_eq!(cleanup["descendants"], expected_descendants);
        match error {
            Some(error) => assert_eq!(cleanup["descendant_error"], error),
            None => assert!(cleanup["descendant_error"].is_null()),
        }
    }
}
