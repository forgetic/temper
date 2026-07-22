//! Bounded, content-free shutdown diagnostics shared by worker, daemon, and
//! standalone composition roots.
//!
//! These DTOs deliberately cannot carry command arguments, output, environment
//! values, or arbitrary error bodies. Runtime tiers stamp assignment identity
//! and timing at the trusted boundary before emitting them.

use serde::{Deserialize, Serialize};

pub const MAX_SHUTDOWN_IDENTIFIER_BYTES: usize = 256;
pub const MAX_SHUTDOWN_ROOT_BYTES: usize = 512;
pub const MAX_SHUTDOWN_SURVIVOR_PIDS: usize = 16;
pub const MAX_SHUTDOWN_BLOCKERS: usize = 64;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownBlockerKind {
    Containment,
    TerminalTraceAck,
    ResultDelivery,
    ComponentTask,
    RegistryState,
}

impl ShutdownBlockerKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Containment => "containment",
            Self::TerminalTraceAck => "terminal_trace_ack",
            Self::ResultDelivery => "result_delivery",
            Self::ComponentTask => "component_task",
            Self::RegistryState => "registry_state",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownEscalationStage {
    Graceful,
    ForcedTermination,
    HardKill,
    EmergencyKill,
}

impl ShutdownEscalationStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Graceful => "graceful",
            Self::ForcedTermination => "forced_termination",
            Self::HardKill => "hard_kill",
            Self::EmergencyKill => "emergency_kill",
        }
    }
}

/// One unresolved shutdown condition. `first_seen_millis` is an
/// observer-provided timestamp; age and deadline fields are monotonic durations
/// and are the only timing values comparable across process boundaries.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShutdownBlocker {
    pub kind: ShutdownBlockerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    pub owner_scope: String,
    pub owner_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub survivor_pids: Vec<u32>,
    #[serde(default)]
    pub omitted_survivor_pids: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub containment_phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_sequence: Option<u64>,
    pub first_seen_millis: u64,
    pub age_millis: u64,
    pub escalation_stage: ShutdownEscalationStage,
    pub deadline_remaining_millis: u64,
    #[serde(default = "one")]
    pub occurrences: u64,
}

impl ShutdownBlocker {
    pub fn new(
        kind: ShutdownBlockerKind,
        escalation_stage: ShutdownEscalationStage,
        owner_scope: impl AsRef<str>,
        owner_name: impl AsRef<str>,
    ) -> Self {
        Self {
            kind,
            worker_id: None,
            job_id: None,
            attempt_id: None,
            owner_scope: safe_shutdown_identifier(owner_scope.as_ref()),
            owner_name: safe_shutdown_identifier(owner_name.as_ref()),
            owner_root: None,
            root_pid: None,
            survivor_pids: Vec::new(),
            omitted_survivor_pids: 0,
            containment_phase: None,
            trace_run_id: None,
            trace_sequence: None,
            first_seen_millis: 0,
            age_millis: 0,
            escalation_stage,
            deadline_remaining_millis: 0,
            occurrences: 1,
        }
    }

    pub fn with_identity(
        mut self,
        worker_id: Option<&str>,
        job_id: Option<&str>,
        attempt_id: Option<&str>,
    ) -> Self {
        self.worker_id = worker_id.map(safe_shutdown_identifier);
        self.job_id = job_id.map(safe_shutdown_identifier);
        self.attempt_id = attempt_id.map(safe_shutdown_identifier);
        self
    }

    pub fn with_containment(
        mut self,
        root: Option<&str>,
        root_pid: Option<u32>,
        phase: Option<&str>,
        survivor_pids: impl IntoIterator<Item = u32>,
        omitted_survivor_pids: u64,
    ) -> Self {
        self.owner_root = root.map(safe_shutdown_root);
        self.root_pid = root_pid.filter(|pid| *pid != 0);
        self.containment_phase = phase.map(safe_shutdown_identifier);
        let mut overflow = 0_u64;
        for pid in survivor_pids {
            if self.survivor_pids.len() < MAX_SHUTDOWN_SURVIVOR_PIDS {
                self.survivor_pids.push(pid);
            } else {
                overflow = overflow.saturating_add(1);
            }
        }
        self.omitted_survivor_pids = omitted_survivor_pids.saturating_add(overflow);
        self
    }

    pub fn with_trace(mut self, run_id: Option<&str>, sequence: Option<u64>) -> Self {
        self.trace_run_id = run_id.map(safe_shutdown_identifier);
        self.trace_sequence = sequence;
        self
    }

    pub fn with_timing(
        mut self,
        first_seen_millis: u64,
        age_millis: u64,
        deadline_remaining_millis: u64,
    ) -> Self {
        self.first_seen_millis = first_seen_millis;
        self.age_millis = age_millis;
        self.deadline_remaining_millis = deadline_remaining_millis;
        self
    }

    /// Re-applies bounds at the final logging boundary. This protects event
    /// emitters even when a caller constructed the public DTO manually.
    pub fn sanitized(&self) -> Self {
        Self {
            kind: self.kind,
            worker_id: self.worker_id.as_deref().map(safe_shutdown_identifier),
            job_id: self.job_id.as_deref().map(safe_shutdown_identifier),
            attempt_id: self.attempt_id.as_deref().map(safe_shutdown_identifier),
            owner_scope: safe_shutdown_identifier(&self.owner_scope),
            owner_name: safe_shutdown_identifier(&self.owner_name),
            owner_root: self.owner_root.as_deref().map(safe_shutdown_root),
            root_pid: self.root_pid.filter(|pid| *pid != 0),
            survivor_pids: self
                .survivor_pids
                .iter()
                .take(MAX_SHUTDOWN_SURVIVOR_PIDS)
                .copied()
                .collect(),
            omitted_survivor_pids: self.omitted_survivor_pids.saturating_add(
                u64::try_from(
                    self.survivor_pids
                        .len()
                        .saturating_sub(MAX_SHUTDOWN_SURVIVOR_PIDS),
                )
                .unwrap_or(u64::MAX),
            ),
            containment_phase: self
                .containment_phase
                .as_deref()
                .map(safe_shutdown_identifier),
            trace_run_id: self.trace_run_id.as_deref().map(safe_shutdown_identifier),
            trace_sequence: self.trace_sequence,
            first_seen_millis: self.first_seen_millis,
            age_millis: self.age_millis,
            escalation_stage: self.escalation_stage,
            deadline_remaining_millis: self.deadline_remaining_millis,
            occurrences: self.occurrences.max(1),
        }
    }
}

pub fn safe_shutdown_identifier(value: &str) -> String {
    safe_shutdown_text(value, MAX_SHUTDOWN_IDENTIFIER_BYTES)
}

pub fn safe_shutdown_root(value: &str) -> String {
    safe_shutdown_text(value, MAX_SHUTDOWN_ROOT_BYTES)
}

const fn one() -> u64 {
    1
}

fn safe_shutdown_text(value: &str, maximum: usize) -> String {
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
    if value.is_empty() {
        return "unknown".to_string();
    }
    if value.len() <= maximum {
        return value.to_string();
    }
    let mut end = maximum;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}
