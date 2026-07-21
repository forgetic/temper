use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityEventV1, AgentRunEventV1, FailureCodeV1, FailureInfoV1,
    RunFailedV1, RunFinishedV1, RunStatusV1, StopReasonV1,
};
use temper_protocol_worker::FailureClass;

use super::{
    TraceError, TraceRun, TraceTerminal, TraceTerminalKind, append_event, elapsed_ms,
    host_failure_summary, now_rfc3339,
};

impl TraceRun {
    /// Writes the sole successful terminal event for the run.
    pub fn finish_success(&self, stop_reason: Option<StopReasonV1>) -> Result<u64, TraceError> {
        let duration_ms = elapsed_ms(self.inner.started);
        self.finish(
            AgentActivityEventV1::RunFinished(RunFinishedV1 {
                status: RunStatusV1::Succeeded,
                duration_ms,
                stop_reason,
            }),
            TraceTerminalKind::Other,
        )
    }

    /// Writes the sole synthetic cancelled terminal event. The worker uses this
    /// after graceful or forced process termination when the child cannot emit
    /// its own terminal frame. Repeating cancellation returns the original
    /// durable sequence so a later orchestration layer can still wait for the
    /// exact terminal acknowledgement.
    pub fn finish_cancelled(&self) -> Result<u64, TraceError> {
        let duration_ms = elapsed_ms(self.inner.started);
        self.finish(
            AgentActivityEventV1::RunFinished(RunFinishedV1 {
                status: RunStatusV1::Cancelled,
                duration_ms,
                stop_reason: Some(StopReasonV1::Cancelled),
            }),
            TraceTerminalKind::Cancelled,
        )
    }

    /// Writes the sole failed/crashed terminal event from trusted host classifications.
    pub fn finish_failure(
        &self,
        code: FailureCodeV1,
        class: FailureClass,
    ) -> Result<u64, TraceError> {
        self.finish(
            AgentActivityEventV1::RunFailed(RunFailedV1 {
                failure: FailureInfoV1 {
                    code,
                    message: host_failure_summary(class).to_string(),
                    retryable: class == FailureClass::Transient,
                },
            }),
            TraceTerminalKind::Other,
        )
    }

    fn finish(
        &self,
        event: AgentActivityEventV1,
        terminal_kind: TraceTerminalKind,
    ) -> Result<u64, TraceError> {
        let mut state = self.inner.state.lock().expect("trace run state lock");
        if let Some(terminal) = state.terminal {
            return if terminal.kind == TraceTerminalKind::Cancelled
                && terminal_kind == TraceTerminalKind::Cancelled
            {
                Ok(terminal.sequence)
            } else {
                Err(TraceError::AlreadyTerminal)
            };
        }
        if state.disabled {
            return Err(TraceError::Disabled);
        }
        let seq = state.next_seq;
        let canonical = AgentRunEventV1 {
            version: ACTIVITY_PROTOCOL_VERSION,
            run_id: self.inner.manifest.run_id.clone(),
            seq,
            occurred_at: now_rfc3339(),
            elapsed_ms: elapsed_ms(self.inner.started),
            assignment: self.inner.manifest.assignment.clone(),
            agent_session_id: self.inner.manifest.agent_session_id.clone(),
            scope: self.inner.manifest.main_scope.clone(),
            turn: None,
            event,
        };
        append_event(&self.inner, &mut state, &canonical, false)?;
        state.terminal = Some(TraceTerminal {
            sequence: seq,
            kind: terminal_kind,
        });
        Ok(seq)
    }
}
