use serde::Serialize;
use temper_protocol_agent::{
    AgentContainmentBackendV1, AgentContainmentDispositionV1, AgentContainmentEventV1,
    AgentContainmentOwnerV1, AgentContainmentPhaseV1, AgentContainmentProcessV1,
    AgentContainmentReapStatusV1, AgentContainmentSignalAttemptV1, AgentContainmentSignalOutcomeV1,
    AgentContainmentTriggerV1,
};

use super::*;

pub(super) fn containment_event_from_lifecycle(
    context: &ContainmentEventContext,
    observation: &AgentContainmentEventV1,
) -> ContainmentEvent {
    match observation {
        AgentContainmentEventV1::CleanupBlocked(event) => {
            ContainmentEvent::CleanupBlocked(CleanupBlocked {
                owner: owner(context, &event.owner),
                trigger: trigger_name_v1(event.trigger),
                phase: phase_name_v1(event.phase),
                repeated_failures: event.repeated_failures,
                term_outcomes: serialize_signal_attempts_v1(&event.term_attempts),
                omitted_term_outcomes: to_usize(event.omitted_term_attempts),
                kill_outcomes: serialize_signal_attempts_v1(&event.kill_attempts),
                omitted_kill_outcomes: to_usize(event.omitted_kill_attempts),
                direct_child_reap: "pending",
                direct_child_pid: 0,
                recursive_empty: "not_proven",
                recursive_empty_inspections: 0,
                survivors: serialize_processes_v1(&event.survivors),
                omitted_survivors: to_usize(event.omitted_survivors),
            })
        }
        AgentContainmentEventV1::CleanupCompleted(event) => {
            ContainmentEvent::CleanupCompleted(CleanupCompleted {
                owner: owner(context, &event.owner),
                trigger: trigger_name_v1(event.trigger),
                disposition: disposition_name_v1(event.disposition),
                term_outcomes: serialize_signal_attempts_v1(&event.term_attempts),
                omitted_term_outcomes: to_usize(event.omitted_term_attempts),
                kill_outcomes: serialize_signal_attempts_v1(&event.kill_attempts),
                omitted_kill_outcomes: to_usize(event.omitted_kill_attempts),
                direct_child_reap: reap_name_v1(event.direct_child_reap),
                direct_child_pid: event.direct_child_pid,
                direct_child_exit_code: event.direct_child_exit_code,
                recursive_empty: "proven",
                recursive_empty_inspections: event.recursive_empty_inspections,
                survivors: serialize_processes_v1(&event.survivors),
                omitted_survivors: to_usize(event.omitted_survivors),
                recovered_inspection_failures: to_usize(event.recovered_inspection_failures),
                omitted_inspection_failures: to_usize(event.omitted_inspection_failures),
            })
        }
        AgentContainmentEventV1::FallbackActivated(event) => {
            ContainmentEvent::ContainmentFallbackActivated(ContainmentFallbackActivated {
                owner: owner(context, &event.owner),
                fallback_reason: bounded_diagnostic(&event.reason, MAX_EVENT_REASON_BYTES),
                term_outcomes: "[]".to_string(),
                kill_outcomes: "[]".to_string(),
                direct_child_reap: "not_started",
                recursive_empty: "not_inspected",
                survivors: "[]".to_string(),
            })
        }
    }
}

pub(super) fn lifecycle_root(observation: &AgentContainmentEventV1) -> &str {
    match observation {
        AgentContainmentEventV1::CleanupBlocked(event) => &event.owner.root,
        AgentContainmentEventV1::CleanupCompleted(event) => &event.owner.root,
        AgentContainmentEventV1::FallbackActivated(event) => &event.owner.root,
    }
}

pub(super) fn lifecycle_repeated_failures(observation: &AgentContainmentEventV1) -> Option<u64> {
    match observation {
        AgentContainmentEventV1::CleanupBlocked(event) => Some(event.repeated_failures),
        _ => None,
    }
}

fn owner(
    context: &ContainmentEventContext,
    owner: &AgentContainmentOwnerV1,
) -> ContainmentEventIdentity {
    ContainmentEventIdentity {
        context: context.clone(),
        owner_kind: bounded_diagnostic(&owner.owner_kind, MAX_EVENT_IDENTIFIER_BYTES),
        tool_command_id: bounded_diagnostic(&owner.tool_command_id, MAX_EVENT_IDENTIFIER_BYTES),
        backend: backend_name_v1(owner.backend),
        root: bounded_diagnostic(&owner.root, MAX_EVENT_ROOT_BYTES),
    }
}

#[derive(Serialize)]
struct SerializableProcessV1 {
    pid: u32,
    ppid: u32,
    pgid: u32,
    session_id: u32,
    start_time: u64,
    executable: String,
}

impl From<&AgentContainmentProcessV1> for SerializableProcessV1 {
    fn from(process: &AgentContainmentProcessV1) -> Self {
        Self {
            pid: process.pid,
            ppid: process.ppid,
            pgid: process.pgid,
            session_id: process.session_id,
            start_time: process.start_time,
            executable: bounded_diagnostic(&process.executable, MAX_EVENT_EXECUTABLE_BYTES),
        }
    }
}

#[derive(Serialize)]
struct SerializableSignalAttemptV1 {
    process: SerializableProcessV1,
    outcome: &'static str,
}

fn serialize_processes_v1(processes: &[AgentContainmentProcessV1]) -> String {
    let bounded = processes
        .iter()
        .take(MAX_EVENT_SURVIVORS)
        .map(SerializableProcessV1::from)
        .collect::<Vec<_>>();
    serde_json::to_string(&bounded).expect("bounded lifecycle process evidence serializes")
}

fn serialize_signal_attempts_v1(attempts: &[AgentContainmentSignalAttemptV1]) -> String {
    let bounded = attempts
        .iter()
        .take(MAX_EVENT_SIGNAL_OUTCOMES)
        .map(|attempt| SerializableSignalAttemptV1 {
            process: SerializableProcessV1::from(&attempt.process),
            outcome: match attempt.outcome {
                AgentContainmentSignalOutcomeV1::Succeeded => "succeeded",
                AgentContainmentSignalOutcomeV1::ProcessGone => "process_gone",
                AgentContainmentSignalOutcomeV1::PidReused => "pid_reused",
                AgentContainmentSignalOutcomeV1::Failed => "failed",
            },
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&bounded).expect("bounded lifecycle signal evidence serializes")
}

const fn backend_name_v1(backend: AgentContainmentBackendV1) -> &'static str {
    match backend {
        AgentContainmentBackendV1::NoProcess => "no_process",
        AgentContainmentBackendV1::LinuxCgroupV2 => "linux_cgroup_v2",
        AgentContainmentBackendV1::LinuxSupervisor => "linux_supervisor",
        AgentContainmentBackendV1::WindowsJob => "windows_job",
    }
}

const fn trigger_name_v1(trigger: AgentContainmentTriggerV1) -> &'static str {
    match trigger {
        AgentContainmentTriggerV1::NormalRootExit => "normal_root_exit",
        AgentContainmentTriggerV1::Timeout => "timeout",
        AgentContainmentTriggerV1::Cancellation => "cancellation",
        AgentContainmentTriggerV1::OwnerDrop => "owner_drop",
        AgentContainmentTriggerV1::Watchdog => "watchdog",
        AgentContainmentTriggerV1::Shutdown => "shutdown",
    }
}

const fn phase_name_v1(phase: AgentContainmentPhaseV1) -> &'static str {
    match phase {
        AgentContainmentPhaseV1::Discover => "discover",
        AgentContainmentPhaseV1::Term => "term",
        AgentContainmentPhaseV1::Grace => "grace",
        AgentContainmentPhaseV1::Kill => "kill",
        AgentContainmentPhaseV1::Reap => "reap",
        AgentContainmentPhaseV1::VerifyEmpty => "verify_empty",
    }
}

const fn disposition_name_v1(disposition: AgentContainmentDispositionV1) -> &'static str {
    match disposition {
        AgentContainmentDispositionV1::AlreadyEmpty => "already_empty",
        AgentContainmentDispositionV1::Terminated => "terminated",
        AgentContainmentDispositionV1::Killed => "killed",
    }
}

const fn reap_name_v1(reap: AgentContainmentReapStatusV1) -> &'static str {
    match reap {
        AgentContainmentReapStatusV1::NotSpawned => "not_spawned",
        AgentContainmentReapStatusV1::Pending => "pending",
        AgentContainmentReapStatusV1::Reaped => "reaped",
        AgentContainmentReapStatusV1::AlreadyReaped => "already_reaped",
    }
}

fn to_usize(value: u64) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}
