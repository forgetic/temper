//! Typed, content-free descendant-cleanup observability.
//!
//! These event types deliberately have no fields for command arguments,
//! prompts, output, environment values, or credentials. Kernel paths and
//! process samples are UTF-8 bounded before they reach `tracing`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use temper_process_containment::{
    CleanupDisposition, CleanupObservation, CleanupPhase, CleanupSnapshot, CleanupTrigger,
    ContainmentBackendKind, ContainmentCapabilityDiagnostic, ContainmentFallbackObservation,
    ContainmentScope, DirectChildReap, ProcessIdentity, RecursiveEmptyProof, SignalAttempt,
    SignalAttemptOutcome,
};

const MAX_EVENT_ROOT_BYTES: usize = 512;
const MAX_EVENT_IDENTIFIER_BYTES: usize = 256;
const MAX_EVENT_REASON_BYTES: usize = 512;
const MAX_EVENT_EXECUTABLE_BYTES: usize = 256;
const MAX_EVENT_SURVIVORS: usize = 16;
const MAX_EVENT_SIGNAL_OUTCOMES: usize = 32;
const DEFAULT_BLOCKED_THROTTLE: Duration = Duration::from_secs(30);

mod lifecycle;
mod render;
mod startup;
use lifecycle::*;
use render::*;
pub(crate) use startup::emit_startup_containment_capability_once;
pub use startup::observe_startup_containment_capability;
#[cfg(test)]
use startup::startup_scavenge_from_parts;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainmentEventContext {
    pub worker_id: String,
    pub job_id: String,
    pub attempt_id: String,
}

impl ContainmentEventContext {
    pub fn new(worker_id: &str, job_id: &str, attempt_id: &str) -> Self {
        Self {
            worker_id: bounded(worker_id, MAX_EVENT_IDENTIFIER_BYTES),
            job_id: bounded(job_id, MAX_EVENT_IDENTIFIER_BYTES),
            attempt_id: bounded(attempt_id, MAX_EVENT_IDENTIFIER_BYTES),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainmentEventIdentity {
    pub context: ContainmentEventContext,
    pub owner_kind: String,
    pub tool_command_id: String,
    pub backend: &'static str,
    pub root: String,
}

impl ContainmentEventIdentity {
    fn cleanup(context: &ContainmentEventContext, observation: &CleanupObservation) -> Self {
        Self {
            context: context.clone(),
            owner_kind: owner_kind(observation.scope()),
            tool_command_id: bounded_diagnostic(
                observation.identity().owner_identifier(),
                MAX_EVENT_IDENTIFIER_BYTES,
            ),
            backend: backend_name(observation.backend()),
            root: bounded_diagnostic(observation.root().value(), MAX_EVENT_ROOT_BYTES),
        }
    }

    fn fallback(
        context: &ContainmentEventContext,
        observation: &ContainmentFallbackObservation,
    ) -> Self {
        Self {
            context: context.clone(),
            owner_kind: owner_kind(observation.scope()),
            tool_command_id: bounded_diagnostic(
                observation.identity().owner_identifier(),
                MAX_EVENT_IDENTIFIER_BYTES,
            ),
            backend: backend_name(observation.backend()),
            root: bounded_diagnostic(observation.root().value(), MAX_EVENT_ROOT_BYTES),
        }
    }
}

/// Non-terminal, fail-closed cleanup evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupBlocked {
    pub owner: ContainmentEventIdentity,
    pub trigger: &'static str,
    pub phase: &'static str,
    pub repeated_failures: u64,
    pub term_outcomes: String,
    pub omitted_term_outcomes: usize,
    pub kill_outcomes: String,
    pub omitted_kill_outcomes: usize,
    pub direct_child_reap: &'static str,
    pub direct_child_pid: u32,
    pub recursive_empty: &'static str,
    pub recursive_empty_inspections: u64,
    pub survivors: String,
    pub omitted_survivors: usize,
}

/// Terminal recursive-empty and direct-child-reap evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupCompleted {
    pub owner: ContainmentEventIdentity,
    pub trigger: &'static str,
    pub disposition: &'static str,
    pub term_outcomes: String,
    pub omitted_term_outcomes: usize,
    pub kill_outcomes: String,
    pub omitted_kill_outcomes: usize,
    pub direct_child_reap: &'static str,
    pub direct_child_pid: u32,
    pub direct_child_exit_code: i64,
    pub recursive_empty: &'static str,
    pub recursive_empty_inspections: u64,
    pub survivors: String,
    pub omitted_survivors: usize,
    pub recovered_inspection_failures: usize,
    pub omitted_inspection_failures: usize,
}

/// Auto-selection evidence for one owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainmentFallbackActivated {
    pub owner: ContainmentEventIdentity,
    pub fallback_reason: String,
    pub term_outcomes: String,
    pub kill_outcomes: String,
    pub direct_child_reap: &'static str,
    pub recursive_empty: &'static str,
    pub survivors: String,
}

/// Stale delegated cgroups inspected before the worker accepts jobs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainmentStartupScavenge {
    pub worker_id: String,
    pub removed_count: usize,
    pub protected_count: usize,
    pub retained_count: usize,
    pub retained_diagnostics: String,
    pub omitted_diagnostics: usize,
}

/// One startup capability and backend-selection diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainmentStartupCapability {
    pub worker_id: String,
    pub cgroup_v2_mount: String,
    pub delegation: bool,
    pub nested_subtree_writable: bool,
    pub cgroup_kill: bool,
    pub pidfd: bool,
    pub selected_backend: &'static str,
    pub fallback_reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContainmentEvent {
    CleanupBlocked(CleanupBlocked),
    CleanupCompleted(CleanupCompleted),
    ContainmentFallbackActivated(ContainmentFallbackActivated),
    StartupCapability(ContainmentStartupCapability),
    StartupScavenge(ContainmentStartupScavenge),
}

impl ContainmentEvent {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::CleanupBlocked(_) => "worker.containment.cleanup_blocked",
            Self::CleanupCompleted(_) => "worker.containment.cleanup_completed",
            Self::ContainmentFallbackActivated(_) => "worker.containment.fallback_activated",
            Self::StartupCapability(_) => "worker.containment.startup_capability",
            Self::StartupScavenge(_) => "worker.containment.startup_scavenge",
        }
    }

    fn from_cleanup(
        context: &ContainmentEventContext,
        observation: &CleanupObservation,
        repeated_failures: u64,
        pending_signals: Option<&PendingSignalEvidence>,
    ) -> Option<Self> {
        let owner = ContainmentEventIdentity::cleanup(context, observation);
        match observation.snapshot() {
            CleanupSnapshot::Blocked {
                trigger,
                phase,
                survivors,
                omitted_survivors,
                ..
            } => Some(Self::CleanupBlocked(CleanupBlocked {
                owner,
                trigger: trigger_name(*trigger),
                phase: phase_name(*phase),
                repeated_failures,
                term_outcomes: pending_signals.map_or_else(
                    || "[]".to_string(),
                    |signals| serialize_signal_attempts(&signals.term),
                ),
                omitted_term_outcomes: pending_signals.map_or(0, |signals| {
                    signals.omitted_term
                        + signals.term.len().saturating_sub(MAX_EVENT_SIGNAL_OUTCOMES)
                }),
                kill_outcomes: pending_signals.map_or_else(
                    || "[]".to_string(),
                    |signals| serialize_signal_attempts(&signals.kill),
                ),
                omitted_kill_outcomes: pending_signals.map_or(0, |signals| {
                    signals.omitted_kill
                        + signals.kill.len().saturating_sub(MAX_EVENT_SIGNAL_OUTCOMES)
                }),
                direct_child_reap: "pending",
                direct_child_pid: 0,
                recursive_empty: "not_proven",
                recursive_empty_inspections: 0,
                survivors: serialize_processes(survivors),
                omitted_survivors: *omitted_survivors
                    + survivors.len().saturating_sub(MAX_EVENT_SURVIVORS),
            })),
            CleanupSnapshot::Completed { report } => {
                let (direct_child_reap, direct_child_pid, direct_child_exit_code) =
                    reap_fields(report.direct_child_reap());
                let (recursive_empty, recursive_empty_inspections) =
                    recursive_empty_fields(report.recursive_empty());
                Some(Self::CleanupCompleted(CleanupCompleted {
                    owner,
                    trigger: trigger_name(report.trigger()),
                    disposition: disposition_name(report.disposition()),
                    term_outcomes: serialize_signal_attempts(report.term_attempts()),
                    omitted_term_outcomes: report.omitted_term_attempts()
                        + report
                            .term_attempts()
                            .len()
                            .saturating_sub(MAX_EVENT_SIGNAL_OUTCOMES),
                    kill_outcomes: serialize_signal_attempts(report.kill_attempts()),
                    omitted_kill_outcomes: report.omitted_kill_attempts()
                        + report
                            .kill_attempts()
                            .len()
                            .saturating_sub(MAX_EVENT_SIGNAL_OUTCOMES),
                    direct_child_reap,
                    direct_child_pid,
                    direct_child_exit_code,
                    recursive_empty,
                    recursive_empty_inspections,
                    survivors: serialize_processes(report.observed_survivors()),
                    omitted_survivors: report.omitted_survivors()
                        + report
                            .observed_survivors()
                            .len()
                            .saturating_sub(MAX_EVENT_SURVIVORS),
                    recovered_inspection_failures: report.blocked_diagnostics().len(),
                    omitted_inspection_failures: report.omitted_blocked_diagnostics(),
                }))
            }
            CleanupSnapshot::Inspecting { .. }
            | CleanupSnapshot::SignalAttempted { .. }
            | CleanupSnapshot::GracePeriod { .. } => None,
        }
    }

    fn from_fallback(
        context: &ContainmentEventContext,
        observation: &ContainmentFallbackObservation,
    ) -> Self {
        Self::ContainmentFallbackActivated(ContainmentFallbackActivated {
            owner: ContainmentEventIdentity::fallback(context, observation),
            fallback_reason: bounded_diagnostic(observation.reason(), MAX_EVENT_REASON_BYTES),
            term_outcomes: "[]".to_string(),
            kill_outcomes: "[]".to_string(),
            direct_child_reap: "not_started",
            recursive_empty: "not_inspected",
            survivors: "[]".to_string(),
        })
    }

    fn startup(worker_id: &str, diagnostic: &ContainmentCapabilityDiagnostic) -> Self {
        Self::StartupCapability(ContainmentStartupCapability {
            worker_id: bounded(worker_id, MAX_EVENT_IDENTIFIER_BYTES),
            cgroup_v2_mount: diagnostic.cgroup_v2_mount().map_or_else(
                || "unavailable".to_string(),
                |path| bounded_diagnostic(path, MAX_EVENT_ROOT_BYTES),
            ),
            delegation: diagnostic.delegation(),
            nested_subtree_writable: diagnostic.nested_subtree_writable(),
            cgroup_kill: diagnostic.cgroup_kill(),
            pidfd: diagnostic.pidfd(),
            selected_backend: backend_name(diagnostic.selected_backend()),
            fallback_reason: diagnostic.fallback_reason().map_or_else(
                || "none".to_string(),
                |reason| bounded_diagnostic(reason, MAX_EVENT_REASON_BYTES),
            ),
        })
    }

    pub fn emit(&self) {
        match self {
            Self::CleanupBlocked(event) => emit_cleanup_blocked(event),
            Self::CleanupCompleted(event) => emit_cleanup_completed(event),
            Self::ContainmentFallbackActivated(event) => emit_fallback(event),
            Self::StartupCapability(event) => emit_startup(event),
            Self::StartupScavenge(event) => emit_startup_scavenge(event),
        }
    }
}

/// Injection seam used by startup and structured-event tests. The production
/// observer is merely an adapter to `tracing`; event construction is testable
/// without installing global logging state.
pub trait ContainmentEventObserver: Send + Sync {
    fn observe(&self, event: &ContainmentEvent);
}

#[derive(Debug, Default)]
pub struct TracingContainmentEventObserver;

impl ContainmentEventObserver for TracingContainmentEventObserver {
    fn observe(&self, event: &ContainmentEvent) {
        event.emit();
    }
}

#[derive(Clone)]
pub(crate) struct ContainmentEventThrottle {
    inner: Arc<ContainmentEventThrottleInner>,
}

struct ContainmentEventThrottleInner {
    observer: Arc<dyn ContainmentEventObserver>,
    interval: Duration,
    blocked: Mutex<HashMap<String, RootObservationState>>,
}

#[derive(Clone, Default)]
struct PendingSignalEvidence {
    term: Vec<SignalAttempt>,
    omitted_term: usize,
    kill: Vec<SignalAttempt>,
    omitted_kill: usize,
}

#[derive(Default)]
struct RootObservationState {
    failures: u64,
    last_emitted: Option<Instant>,
    signals: PendingSignalEvidence,
}

impl Default for ContainmentEventThrottle {
    fn default() -> Self {
        Self::new(
            Arc::new(TracingContainmentEventObserver),
            DEFAULT_BLOCKED_THROTTLE,
        )
    }
}

impl ContainmentEventThrottle {
    pub(crate) fn new(observer: Arc<dyn ContainmentEventObserver>, interval: Duration) -> Self {
        Self {
            inner: Arc::new(ContainmentEventThrottleInner {
                observer,
                interval,
                blocked: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub(crate) fn cleanup(
        &self,
        context: &ContainmentEventContext,
        observation: &CleanupObservation,
    ) {
        let root = observation.root().value().to_string();
        let (repeated_failures, pending_signals) = match observation.snapshot() {
            CleanupSnapshot::SignalAttempted {
                signal,
                attempts,
                omitted,
                ..
            } => {
                let mut states = self
                    .inner
                    .blocked
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let signals = &mut states.entry(root).or_default().signals;
                let (retained, omitted_total) = match signal {
                    temper_process_containment::ContainmentSignal::Term => {
                        (&mut signals.term, &mut signals.omitted_term)
                    }
                    temper_process_containment::ContainmentSignal::Kill => {
                        (&mut signals.kill, &mut signals.omitted_kill)
                    }
                };
                let remaining = MAX_EVENT_SIGNAL_OUTCOMES.saturating_sub(retained.len());
                retained.extend(attempts.iter().take(remaining).cloned());
                *omitted_total = omitted_total
                    .saturating_add(*omitted)
                    .saturating_add(attempts.len().saturating_sub(remaining));
                return;
            }
            CleanupSnapshot::Blocked { .. } => {
                let now = Instant::now();
                let mut states = self
                    .inner
                    .blocked
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let state = states.entry(root).or_default();
                state.failures = state.failures.saturating_add(1);
                let promoted = state.failures == 3;
                let due = state
                    .last_emitted
                    .is_none_or(|last| now.duration_since(last) >= self.inner.interval);
                if !promoted && !due {
                    return;
                }
                state.last_emitted = Some(now);
                (state.failures, Some(state.signals.clone()))
            }
            CleanupSnapshot::Completed { .. } => {
                self.inner
                    .blocked
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&root);
                (0, None)
            }
            _ => (0, None),
        };
        if let Some(event) = ContainmentEvent::from_cleanup(
            context,
            observation,
            repeated_failures,
            pending_signals.as_ref(),
        ) {
            self.inner.observer.observe(&event);
        }
    }

    pub(crate) fn lifecycle(
        &self,
        context: &ContainmentEventContext,
        observation: &temper_protocol_agent::AgentContainmentEventV1,
    ) {
        let root = lifecycle_root(observation).to_string();
        if let Some(reported_failures) = lifecycle_repeated_failures(observation) {
            let now = Instant::now();
            let mut states = self
                .inner
                .blocked
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let state = states.entry(root).or_default();
            let promoted = state.failures < 3 && reported_failures >= 3;
            let due = state
                .last_emitted
                .is_none_or(|last| now.duration_since(last) >= self.inner.interval);
            state.failures = state.failures.max(reported_failures);
            if !promoted && !due {
                return;
            }
            state.last_emitted = Some(now);
        } else if matches!(
            observation,
            temper_protocol_agent::AgentContainmentEventV1::CleanupCompleted(_)
        ) {
            self.inner
                .blocked
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&root);
        }
        self.inner
            .observer
            .observe(&containment_event_from_lifecycle(context, observation));
    }

    pub(crate) fn fallback(
        &self,
        context: &ContainmentEventContext,
        observation: &ContainmentFallbackObservation,
    ) {
        self.inner
            .observer
            .observe(&ContainmentEvent::from_fallback(context, observation));
    }
}

#[cfg(test)]
mod tests;
