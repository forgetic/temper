//! Projection of nested managed-process cleanup onto the always-on lifecycle
//! channel. The projection retains only bounded, content-free kernel evidence;
//! commands, arguments, output, cleanup error strings, and credentials never
//! enter the wire payload.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use temper_agent_core::{
    CleanupDisposition, CleanupObservation, CleanupObserver, CleanupPhase, CleanupSnapshot,
    CleanupTrigger, ContainmentBackendKind, ContainmentFallbackObservation, ContainmentScope,
    ContainmentSignal, DirectChildReap, ProcessIdentity, RecursiveEmptyProof, SignalAttempt,
    SignalAttemptOutcome,
};
use temper_protocol_agent::{
    AgentContainmentBackendV1, AgentContainmentCleanupBlockedV1,
    AgentContainmentCleanupCompletedV1, AgentContainmentDispositionV1, AgentContainmentEventV1,
    AgentContainmentFallbackV1, AgentContainmentOwnerV1, AgentContainmentPhaseV1,
    AgentContainmentProcessV1, AgentContainmentReapStatusV1, AgentContainmentSignalAttemptV1,
    AgentContainmentSignalOutcomeV1, AgentContainmentTriggerV1, AgentLifecycleEventV1,
    AgentLifecycleScopeV1, MAX_AGENT_CONTAINMENT_EXECUTABLE_BYTES,
    MAX_AGENT_CONTAINMENT_IDENTIFIER_BYTES, MAX_AGENT_CONTAINMENT_REASON_BYTES,
    MAX_AGENT_CONTAINMENT_ROOT_BYTES, MAX_AGENT_CONTAINMENT_SIGNAL_ATTEMPTS,
    MAX_AGENT_CONTAINMENT_SURVIVORS,
};

use super::lifecycle::LifecycleProjection;

const DEFAULT_BLOCKED_THROTTLE: Duration = Duration::from_secs(30);

#[derive(Clone, Default)]
struct PendingSignals {
    term: Vec<AgentContainmentSignalAttemptV1>,
    omitted_term: u64,
    kill: Vec<AgentContainmentSignalAttemptV1>,
    omitted_kill: u64,
}

#[derive(Default)]
struct RootState {
    failures: u64,
    last_emitted: Option<Instant>,
    signals: PendingSignals,
}

pub(super) struct LifecycleCleanupObserver {
    projection: Arc<dyn LifecycleProjection>,
    scope: AgentLifecycleScopeV1,
    interval: Duration,
    roots: Mutex<HashMap<String, RootState>>,
}

impl LifecycleCleanupObserver {
    pub(super) fn new(
        projection: Arc<dyn LifecycleProjection>,
        scope: AgentLifecycleScopeV1,
    ) -> Self {
        Self::with_interval(projection, scope, DEFAULT_BLOCKED_THROTTLE)
    }

    fn with_interval(
        projection: Arc<dyn LifecycleProjection>,
        scope: AgentLifecycleScopeV1,
        interval: Duration,
    ) -> Self {
        Self {
            projection,
            scope,
            interval,
            roots: Mutex::new(HashMap::new()),
        }
    }

    fn emit(&self, observation: AgentContainmentEventV1) {
        self.projection.emit(
            self.scope.clone(),
            AgentLifecycleEventV1::Containment { observation },
        );
    }

    fn cleanup(&self, observation: &CleanupObservation) {
        let root = observation.root().value().to_string();
        match observation.snapshot() {
            CleanupSnapshot::SignalAttempted {
                signal,
                attempts,
                omitted,
                ..
            } => {
                let mut roots = self
                    .roots
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let pending = &mut roots.entry(root).or_default().signals;
                let (retained, omitted_total) = match signal {
                    ContainmentSignal::Term => (&mut pending.term, &mut pending.omitted_term),
                    ContainmentSignal::Kill => (&mut pending.kill, &mut pending.omitted_kill),
                };
                append_signal_attempts(retained, omitted_total, attempts, *omitted);
            }
            CleanupSnapshot::Blocked {
                trigger,
                phase,
                survivors,
                omitted_survivors,
                ..
            } => {
                let now = Instant::now();
                let (repeated_failures, signals) = {
                    let mut roots = self
                        .roots
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let state = roots.entry(root).or_default();
                    state.failures = state.failures.saturating_add(1);
                    let promoted = state.failures == 3;
                    let due = state
                        .last_emitted
                        .is_none_or(|last| now.duration_since(last) >= self.interval);
                    if !promoted && !due {
                        return;
                    }
                    state.last_emitted = Some(now);
                    (state.failures, state.signals.clone())
                };
                let (survivors, survivor_overflow) = process_sample(survivors);
                self.emit(AgentContainmentEventV1::CleanupBlocked(
                    AgentContainmentCleanupBlockedV1 {
                        owner: owner(observation),
                        trigger: trigger_v1(*trigger),
                        phase: phase_v1(*phase),
                        repeated_failures,
                        term_attempts: signals.term,
                        omitted_term_attempts: signals.omitted_term,
                        kill_attempts: signals.kill,
                        omitted_kill_attempts: signals.omitted_kill,
                        survivors,
                        omitted_survivors: to_u64(*omitted_survivors)
                            .saturating_add(survivor_overflow),
                    },
                ));
            }
            CleanupSnapshot::Completed { report } => {
                self.roots
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&root);
                let (term_attempts, term_overflow) = signal_sample(report.term_attempts());
                let (kill_attempts, kill_overflow) = signal_sample(report.kill_attempts());
                let (survivors, survivor_overflow) = process_sample(report.observed_survivors());
                let (direct_child_reap, direct_child_pid, direct_child_exit_code) =
                    reap_fields(report.direct_child_reap());
                let recursive_empty_inspections = match report.recursive_empty() {
                    RecursiveEmptyProof::Proven { inspections } => *inspections,
                    RecursiveEmptyProof::NotEmpty { .. } => 0,
                };
                self.emit(AgentContainmentEventV1::CleanupCompleted(
                    AgentContainmentCleanupCompletedV1 {
                        owner: owner(observation),
                        trigger: trigger_v1(report.trigger()),
                        disposition: disposition_v1(report.disposition()),
                        term_attempts,
                        omitted_term_attempts: to_u64(report.omitted_term_attempts())
                            .saturating_add(term_overflow),
                        kill_attempts,
                        omitted_kill_attempts: to_u64(report.omitted_kill_attempts())
                            .saturating_add(kill_overflow),
                        direct_child_reap,
                        direct_child_pid,
                        direct_child_exit_code,
                        recursive_empty_inspections,
                        survivors,
                        omitted_survivors: to_u64(report.omitted_survivors())
                            .saturating_add(survivor_overflow),
                        recovered_inspection_failures: to_u64(report.blocked_diagnostics().len()),
                        omitted_inspection_failures: to_u64(report.omitted_blocked_diagnostics()),
                    },
                ));
            }
            CleanupSnapshot::Inspecting { .. } | CleanupSnapshot::GracePeriod { .. } => {}
        }
    }
}

impl CleanupObserver for LifecycleCleanupObserver {
    fn observe(&self, _snapshot: &CleanupSnapshot) {}

    fn observe_cleanup(&self, observation: &CleanupObservation) {
        self.cleanup(observation);
    }

    fn observe_fallback(&self, fallback: &ContainmentFallbackObservation) {
        self.emit(AgentContainmentEventV1::FallbackActivated(
            AgentContainmentFallbackV1 {
                owner: fallback_owner(fallback),
                reason: safe_diagnostic(fallback.reason(), MAX_AGENT_CONTAINMENT_REASON_BYTES),
            },
        ));
    }
}

fn append_signal_attempts(
    retained: &mut Vec<AgentContainmentSignalAttemptV1>,
    omitted_total: &mut u64,
    attempts: &[SignalAttempt],
    omitted: usize,
) {
    let remaining = MAX_AGENT_CONTAINMENT_SIGNAL_ATTEMPTS.saturating_sub(retained.len());
    retained.extend(attempts.iter().take(remaining).map(signal_attempt));
    *omitted_total = omitted_total
        .saturating_add(to_u64(omitted))
        .saturating_add(to_u64(attempts.len().saturating_sub(remaining)));
}

fn signal_sample(attempts: &[SignalAttempt]) -> (Vec<AgentContainmentSignalAttemptV1>, u64) {
    (
        attempts
            .iter()
            .take(MAX_AGENT_CONTAINMENT_SIGNAL_ATTEMPTS)
            .map(signal_attempt)
            .collect(),
        to_u64(
            attempts
                .len()
                .saturating_sub(MAX_AGENT_CONTAINMENT_SIGNAL_ATTEMPTS),
        ),
    )
}

fn signal_attempt(attempt: &SignalAttempt) -> AgentContainmentSignalAttemptV1 {
    AgentContainmentSignalAttemptV1 {
        process: process(attempt.process()),
        outcome: match attempt.outcome() {
            SignalAttemptOutcome::Succeeded => AgentContainmentSignalOutcomeV1::Succeeded,
            SignalAttemptOutcome::ProcessGone => AgentContainmentSignalOutcomeV1::ProcessGone,
            SignalAttemptOutcome::PidReused => AgentContainmentSignalOutcomeV1::PidReused,
            // Backend error bodies may contain command/environment content.
            SignalAttemptOutcome::Failed(_) => AgentContainmentSignalOutcomeV1::Failed,
        },
    }
}

fn process_sample(processes: &[ProcessIdentity]) -> (Vec<AgentContainmentProcessV1>, u64) {
    (
        processes
            .iter()
            .take(MAX_AGENT_CONTAINMENT_SURVIVORS)
            .map(process)
            .collect(),
        to_u64(
            processes
                .len()
                .saturating_sub(MAX_AGENT_CONTAINMENT_SURVIVORS),
        ),
    )
}

fn process(process: &ProcessIdentity) -> AgentContainmentProcessV1 {
    AgentContainmentProcessV1 {
        pid: process.pid(),
        ppid: process.ppid(),
        pgid: process.process_group_id(),
        session_id: process.session_id(),
        start_time: process.start_time_identity(),
        executable: safe_diagnostic(
            &process.executable().to_string_lossy(),
            MAX_AGENT_CONTAINMENT_EXECUTABLE_BYTES,
        ),
    }
}

fn owner(observation: &CleanupObservation) -> AgentContainmentOwnerV1 {
    AgentContainmentOwnerV1 {
        owner_kind: safe_diagnostic(
            &scope_name(observation.scope()),
            MAX_AGENT_CONTAINMENT_IDENTIFIER_BYTES,
        ),
        tool_command_id: safe_diagnostic(
            observation.identity().owner_identifier(),
            MAX_AGENT_CONTAINMENT_IDENTIFIER_BYTES,
        ),
        backend: backend_v1(observation.backend()),
        root: safe_diagnostic(observation.root().value(), MAX_AGENT_CONTAINMENT_ROOT_BYTES),
        root_pid: (observation.root_pid() != 0).then_some(observation.root_pid()),
    }
}

fn fallback_owner(observation: &ContainmentFallbackObservation) -> AgentContainmentOwnerV1 {
    AgentContainmentOwnerV1 {
        owner_kind: safe_diagnostic(
            &scope_name(observation.scope()),
            MAX_AGENT_CONTAINMENT_IDENTIFIER_BYTES,
        ),
        tool_command_id: safe_diagnostic(
            observation.identity().owner_identifier(),
            MAX_AGENT_CONTAINMENT_IDENTIFIER_BYTES,
        ),
        backend: backend_v1(observation.backend()),
        root: safe_diagnostic(observation.root().value(), MAX_AGENT_CONTAINMENT_ROOT_BYTES),
        root_pid: None,
    }
}

fn scope_name(scope: &ContainmentScope) -> String {
    match scope {
        ContainmentScope::Job => "job".to_string(),
        ContainmentScope::Tool => "tool".to_string(),
        ContainmentScope::Agent => "agent".to_string(),
        ContainmentScope::McpServer => "mcp_server".to_string(),
        ContainmentScope::WorkerCommand => "worker_command".to_string(),
        ContainmentScope::PrePush => "pre_push".to_string(),
        ContainmentScope::Custom(name) => name.clone(),
    }
}

fn backend_v1(backend: ContainmentBackendKind) -> AgentContainmentBackendV1 {
    match backend {
        ContainmentBackendKind::NoProcess => AgentContainmentBackendV1::NoProcess,
        ContainmentBackendKind::LinuxCgroupV2 => AgentContainmentBackendV1::LinuxCgroupV2,
        ContainmentBackendKind::LinuxSupervisor => AgentContainmentBackendV1::LinuxSupervisor,
        ContainmentBackendKind::WindowsJob => AgentContainmentBackendV1::WindowsJob,
    }
}

fn trigger_v1(trigger: CleanupTrigger) -> AgentContainmentTriggerV1 {
    match trigger {
        CleanupTrigger::NormalRootExit => AgentContainmentTriggerV1::NormalRootExit,
        CleanupTrigger::Timeout => AgentContainmentTriggerV1::Timeout,
        CleanupTrigger::Cancellation => AgentContainmentTriggerV1::Cancellation,
        CleanupTrigger::OwnerDrop => AgentContainmentTriggerV1::OwnerDrop,
        CleanupTrigger::Watchdog => AgentContainmentTriggerV1::Watchdog,
        CleanupTrigger::Shutdown => AgentContainmentTriggerV1::Shutdown,
    }
}

fn phase_v1(phase: CleanupPhase) -> AgentContainmentPhaseV1 {
    match phase {
        CleanupPhase::Discover => AgentContainmentPhaseV1::Discover,
        CleanupPhase::Term => AgentContainmentPhaseV1::Term,
        CleanupPhase::Grace => AgentContainmentPhaseV1::Grace,
        CleanupPhase::Kill => AgentContainmentPhaseV1::Kill,
        CleanupPhase::Reap => AgentContainmentPhaseV1::Reap,
        CleanupPhase::VerifyEmpty => AgentContainmentPhaseV1::VerifyEmpty,
    }
}

fn disposition_v1(disposition: CleanupDisposition) -> AgentContainmentDispositionV1 {
    match disposition {
        CleanupDisposition::AlreadyEmpty => AgentContainmentDispositionV1::AlreadyEmpty,
        CleanupDisposition::Terminated => AgentContainmentDispositionV1::Terminated,
        CleanupDisposition::Killed => AgentContainmentDispositionV1::Killed,
    }
}

fn reap_fields(reap: &DirectChildReap) -> (AgentContainmentReapStatusV1, u32, i64) {
    match reap {
        DirectChildReap::NotSpawned => (AgentContainmentReapStatusV1::NotSpawned, 0, -1),
        DirectChildReap::Pending { pid } => (AgentContainmentReapStatusV1::Pending, *pid, -1),
        DirectChildReap::Reaped { pid, exit_code } => (
            AgentContainmentReapStatusV1::Reaped,
            *pid,
            exit_code.map_or(-1, i64::from),
        ),
        DirectChildReap::AlreadyReaped { pid, exit_code } => (
            AgentContainmentReapStatusV1::AlreadyReaped,
            *pid,
            exit_code.map_or(-1, i64::from),
        ),
    }
}

fn safe_diagnostic(value: &str, limit: usize) -> String {
    let lower = value.to_ascii_lowercase();
    if [
        "secret-token-sentinel",
        "authorization:",
        "bearer ",
        "credential=",
        "password=",
        "token=",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return "[redacted]".to_string();
    }
    bounded(value, limit)
}

fn bounded(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
