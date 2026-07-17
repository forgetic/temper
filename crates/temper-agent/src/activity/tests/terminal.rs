use std::sync::Arc;

use temper_agent_core::AgentStop;

use super::*;

#[test]
fn every_agent_stop_projects_a_consistent_scope_status_and_terminal_reason() {
    let cases = [
        (
            AgentStop::Completed,
            ScopeStatusV1::Succeeded,
            AgentTerminalReasonV1::Completed,
        ),
        (
            AgentStop::ModelError,
            ScopeStatusV1::Failed,
            AgentTerminalReasonV1::ModelError,
        ),
        (
            AgentStop::Aborted,
            ScopeStatusV1::Cancelled,
            AgentTerminalReasonV1::Aborted,
        ),
        (
            AgentStop::BudgetExhausted,
            ScopeStatusV1::Failed,
            AgentTerminalReasonV1::BudgetExhausted,
        ),
    ];

    for (stop, expected_status, expected_reason) in cases {
        let recorder = Arc::new(Recorder::default());
        let factory = ScopeFactory::with_parts(
            AgentActivityCapturePolicyV1::default(),
            Arc::new(FakeClock::new([0, 0, 25, 25])),
            vec![recorder.clone()],
        );
        let run = factory.main("main", ModelIdentity::new("provider", "model"));
        run.observability
            .events
            .emit(AgentEvent::AgentEnd { reason: stop });

        let frames = recorder.0.lock().expect("frames");
        let finished = frames
            .iter()
            .find_map(|frame| match &frame.event {
                AgentActivityEventV1::ScopeFinished(finished) => Some(finished),
                _ => None,
            })
            .expect("scope.finished boundary");
        assert_eq!(finished.status, expected_status);
        assert_eq!(finished.terminal_reason, Some(expected_reason));
        assert_eq!(finished.duration_ms, 25);
    }
}
