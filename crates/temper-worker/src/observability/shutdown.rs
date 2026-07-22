//! Standalone shutdown event DTOs. Construction re-applies all shared bounds so
//! the final operator log cannot expose process arguments, output, or secrets.

use temper_protocol_worker::{MAX_SHUTDOWN_BLOCKERS, ShutdownBlocker};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StandaloneShutdownDisposition {
    GracefulExit,
    BoundedCrashHandoff,
}

impl StandaloneShutdownDisposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GracefulExit => "graceful_exit",
            Self::BoundedCrashHandoff => "bounded_crash_handoff",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StandaloneShutdownBlockerEvent {
    pub blocker: ShutdownBlocker,
}

impl StandaloneShutdownBlockerEvent {
    pub fn new(blocker: ShutdownBlocker) -> Self {
        Self {
            blocker: blocker.sanitized(),
        }
    }

    pub fn emit(&self) {
        let blocker = &self.blocker;
        let survivor_pids = serde_json::to_string(&blocker.survivor_pids)
            .expect("bounded shutdown survivor PIDs serialize");
        tracing::error!(
            target: "temper::standalone",
            service = "standalone",
            event = "standalone.shutdown.blocker",
            blocker_kind = blocker.kind.as_str(),
            worker_id = blocker.worker_id.as_deref().unwrap_or("unknown"),
            job_id = blocker.job_id.as_deref().unwrap_or("unknown"),
            attempt_id = blocker.attempt_id.as_deref().unwrap_or("unknown"),
            owner_scope = blocker.owner_scope.as_str(),
            owner_name = blocker.owner_name.as_str(),
            owner_root = blocker.owner_root.as_deref().unwrap_or("unknown"),
            root_pid = blocker.root_pid.unwrap_or_default(),
            survivor_pids = survivor_pids.as_str(),
            omitted_survivor_pids = blocker.omitted_survivor_pids,
            containment_phase = blocker.containment_phase.as_deref().unwrap_or("unknown"),
            trace_run_id = blocker.trace_run_id.as_deref().unwrap_or("unknown"),
            trace_sequence = blocker.trace_sequence.unwrap_or_default(),
            first_seen_millis = blocker.first_seen_millis,
            age_millis = blocker.age_millis,
            escalation_stage = blocker.escalation_stage.as_str(),
            deadline_remaining_millis = blocker.deadline_remaining_millis,
            occurrences = blocker.occurrences,
            "standalone shutdown remains blocked"
        );
    }
}

/// Bounded blocker rollup suitable for the terminal standalone disposition
/// event. Individual blocker events remain the detailed source of truth.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StandaloneShutdownSummaryEvent {
    pub disposition: StandaloneShutdownDisposition,
    pub blockers: Vec<ShutdownBlocker>,
    pub omitted_blockers: usize,
}

impl StandaloneShutdownSummaryEvent {
    pub fn new(
        disposition: StandaloneShutdownDisposition,
        blockers: impl IntoIterator<Item = ShutdownBlocker>,
    ) -> Self {
        let mut retained = Vec::with_capacity(MAX_SHUTDOWN_BLOCKERS);
        let mut omitted_blockers = 0_usize;
        for blocker in blockers {
            if retained.len() < MAX_SHUTDOWN_BLOCKERS {
                retained.push(blocker.sanitized());
            } else {
                omitted_blockers = omitted_blockers.saturating_add(1);
            }
        }
        Self {
            disposition,
            omitted_blockers,
            blockers: retained,
        }
    }

    pub fn emit(&self) {
        let blockers = serde_json::to_string(&self.blockers)
            .expect("bounded shutdown blocker summary serializes");
        match self.disposition {
            StandaloneShutdownDisposition::GracefulExit => tracing::info!(
                target: "temper::standalone",
                service = "standalone",
                event = "standalone.shutdown.summary",
                disposition = self.disposition.as_str(),
                blocker_count = self.blockers.len(),
                omitted_blockers = self.omitted_blockers,
                blockers = blockers.as_str(),
                "standalone shutdown reached its terminal disposition"
            ),
            StandaloneShutdownDisposition::BoundedCrashHandoff => tracing::error!(
                target: "temper::standalone",
                service = "standalone",
                event = "standalone.shutdown.summary",
                disposition = self.disposition.as_str(),
                blocker_count = self.blockers.len(),
                omitted_blockers = self.omitted_blockers,
                blockers = blockers.as_str(),
                "standalone shutdown reached its terminal disposition"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use temper_protocol_worker::{
        MAX_SHUTDOWN_IDENTIFIER_BYTES, MAX_SHUTDOWN_SURVIVOR_PIDS, ShutdownBlockerKind,
        ShutdownEscalationStage,
    };

    use super::*;

    #[test]
    fn blocker_and_summary_dtos_reapply_redaction_and_bounds() {
        let mut blocker = ShutdownBlocker::new(
            ShutdownBlockerKind::Containment,
            ShutdownEscalationStage::EmergencyKill,
            "tool",
            "bash",
        );
        blocker.worker_id = Some("credential=secret-token-sentinel".to_string());
        blocker.owner_name = "x".repeat(MAX_SHUTDOWN_IDENTIFIER_BYTES + 20);
        blocker.owner_root = Some("authorization: bearer secret-token-sentinel".to_string());
        blocker.survivor_pids =
            (1..=u32::try_from(MAX_SHUTDOWN_SURVIVOR_PIDS + 7).unwrap()).collect();

        let event = StandaloneShutdownBlockerEvent::new(blocker.clone());
        assert_eq!(event.blocker.worker_id.as_deref(), Some("[redacted]"));
        assert_eq!(event.blocker.owner_root.as_deref(), Some("[redacted]"));
        assert_eq!(
            event.blocker.owner_name.len(),
            MAX_SHUTDOWN_IDENTIFIER_BYTES
        );
        assert_eq!(
            event.blocker.survivor_pids.len(),
            MAX_SHUTDOWN_SURVIVOR_PIDS
        );
        assert_eq!(event.blocker.omitted_survivor_pids, 7);

        let summary = StandaloneShutdownSummaryEvent::new(
            StandaloneShutdownDisposition::BoundedCrashHandoff,
            std::iter::repeat_n(blocker, MAX_SHUTDOWN_BLOCKERS + 3),
        );
        assert_eq!(summary.blockers.len(), MAX_SHUTDOWN_BLOCKERS);
        assert_eq!(summary.omitted_blockers, 3);
        let encoded = serde_json::to_string(&summary.blockers).unwrap();
        assert!(!encoded.contains("secret-token-sentinel"));
        assert!(!encoded.contains("authorization:"));
    }
}
