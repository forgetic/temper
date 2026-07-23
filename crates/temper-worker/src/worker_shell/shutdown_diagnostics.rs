//! Projection of attempt-local shutdown observations into the shared bounded
//! blocker vocabulary.

use temper_protocol_worker::{ShutdownBlocker, ShutdownBlockerKind, ShutdownEscalationStage};

use crate::executor::TerminalTraceBlocker;

pub(super) fn terminal_trace_shutdown_blocker(blocker: &TerminalTraceBlocker) -> ShutdownBlocker {
    ShutdownBlocker::new(
        ShutdownBlockerKind::TerminalTraceAck,
        ShutdownEscalationStage::Graceful,
        "agent_trace",
        blocker.state().as_str(),
    )
    .with_trace(blocker.run_id(), blocker.sequence())
}

pub(super) fn containment_shutdown_blocker(
    observation: &temper_process_containment::CleanupObservation,
) -> Option<ShutdownBlocker> {
    let temper_process_containment::CleanupSnapshot::Blocked {
        phase,
        survivors,
        omitted_survivors,
        ..
    } = observation.snapshot()
    else {
        return None;
    };
    Some(
        ShutdownBlocker::new(
            ShutdownBlockerKind::Containment,
            ShutdownEscalationStage::Graceful,
            containment_scope_name(observation.scope()),
            observation.identity().owner_identifier(),
        )
        .with_containment(
            Some(observation.root().value()),
            (observation.root_pid() != 0).then_some(observation.root_pid()),
            Some(containment_phase_name(*phase)),
            survivors.iter().map(|process| process.pid()),
            u64::try_from(*omitted_survivors).unwrap_or(u64::MAX),
        ),
    )
}

pub(super) fn lifecycle_shutdown_blocker(
    observation: &temper_protocol_agent::AgentContainmentEventV1,
) -> Option<ShutdownBlocker> {
    let temper_protocol_agent::AgentContainmentEventV1::CleanupBlocked(event) = observation else {
        return None;
    };
    Some(
        ShutdownBlocker::new(
            ShutdownBlockerKind::Containment,
            ShutdownEscalationStage::Graceful,
            &event.owner.owner_kind,
            &event.owner.tool_command_id,
        )
        .with_containment(
            Some(&event.owner.root),
            event.owner.root_pid,
            Some(containment_phase_name_v1(event.phase)),
            event.survivors.iter().map(|process| process.pid),
            event.omitted_survivors,
        ),
    )
}

fn containment_scope_name(scope: &temper_process_containment::ContainmentScope) -> &str {
    match scope {
        temper_process_containment::ContainmentScope::Job => "job",
        temper_process_containment::ContainmentScope::Tool => "tool",
        temper_process_containment::ContainmentScope::Agent => "agent",
        temper_process_containment::ContainmentScope::McpServer => "mcp_server",
        temper_process_containment::ContainmentScope::WorkerCommand => "worker_command",
        temper_process_containment::ContainmentScope::PrePush => "pre_push",
        temper_process_containment::ContainmentScope::Custom(name) => name,
    }
}

fn containment_phase_name(phase: temper_process_containment::CleanupPhase) -> &'static str {
    match phase {
        temper_process_containment::CleanupPhase::Discover => "discover",
        temper_process_containment::CleanupPhase::Term => "term",
        temper_process_containment::CleanupPhase::Grace => "grace",
        temper_process_containment::CleanupPhase::Kill => "kill",
        temper_process_containment::CleanupPhase::Reap => "reap",
        temper_process_containment::CleanupPhase::VerifyEmpty => "verify_empty",
    }
}

fn containment_phase_name_v1(
    phase: temper_protocol_agent::AgentContainmentPhaseV1,
) -> &'static str {
    match phase {
        temper_protocol_agent::AgentContainmentPhaseV1::Discover => "discover",
        temper_protocol_agent::AgentContainmentPhaseV1::Term => "term",
        temper_protocol_agent::AgentContainmentPhaseV1::Grace => "grace",
        temper_protocol_agent::AgentContainmentPhaseV1::Kill => "kill",
        temper_protocol_agent::AgentContainmentPhaseV1::Reap => "reap",
        temper_protocol_agent::AgentContainmentPhaseV1::VerifyEmpty => "verify_empty",
    }
}
