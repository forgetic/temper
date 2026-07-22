//! Typed, content-free terminal-trace shutdown evidence.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalTraceBlockerState {
    AwaitingAcknowledgement,
    PersistenceFailed,
    TraceUnavailable,
    Compatibility,
}

impl TerminalTraceBlockerState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingAcknowledgement => "awaiting_acknowledgement",
            Self::PersistenceFailed => "persistence_failed",
            Self::TraceUnavailable => "trace_unavailable",
            Self::Compatibility => "quiescence_pending",
        }
    }
}

/// Typed terminal-trace evidence retained while cancellation quiescence is
/// fenced. Arbitrary persistence errors never enter this shape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalTraceBlocker {
    run_id: Option<String>,
    sequence: Option<u64>,
    state: TerminalTraceBlockerState,
    compatibility_reason: Option<String>,
}

impl TerminalTraceBlocker {
    pub fn awaiting_acknowledgement(run_id: &str, sequence: u64) -> Self {
        Self::new(
            Some(run_id),
            Some(sequence),
            TerminalTraceBlockerState::AwaitingAcknowledgement,
        )
    }

    pub fn persistence_failed(run_id: &str) -> Self {
        Self::new(
            Some(run_id),
            None,
            TerminalTraceBlockerState::PersistenceFailed,
        )
    }

    pub fn trace_unavailable() -> Self {
        Self::new(None, None, TerminalTraceBlockerState::TraceUnavailable)
    }

    fn new(run_id: Option<&str>, sequence: Option<u64>, state: TerminalTraceBlockerState) -> Self {
        Self {
            run_id: run_id.map(temper_protocol_worker::safe_shutdown_identifier),
            sequence,
            state,
            compatibility_reason: None,
        }
    }

    pub(super) fn compatibility(reason: &str) -> Self {
        Self {
            run_id: None,
            sequence: None,
            state: TerminalTraceBlockerState::Compatibility,
            compatibility_reason: Some(temper_protocol_worker::safe_shutdown_identifier(reason)),
        }
    }

    pub fn run_id(&self) -> Option<&str> {
        self.run_id.as_deref()
    }

    pub fn sequence(&self) -> Option<u64> {
        self.sequence
    }

    pub fn state(&self) -> TerminalTraceBlockerState {
        self.state
    }
}

impl std::fmt::Display for TerminalTraceBlocker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.run_id(), self.sequence(), self.state) {
            (Some(run_id), Some(sequence), TerminalTraceBlockerState::AwaitingAcknowledgement) => {
                write!(
                    formatter,
                    "terminal trace {run_id} sequence {sequence} is awaiting durable acknowledgement"
                )
            }
            (Some(run_id), _, TerminalTraceBlockerState::PersistenceFailed) => {
                write!(
                    formatter,
                    "cancelled terminal trace {run_id} could not be persisted"
                )
            }
            (_, _, TerminalTraceBlockerState::TraceUnavailable) => {
                formatter.write_str("enabled durable tracing did not create a cancellation run")
            }
            _ => formatter.write_str(
                self.compatibility_reason
                    .as_deref()
                    .unwrap_or("attempt quiescence is pending"),
            ),
        }
    }
}
