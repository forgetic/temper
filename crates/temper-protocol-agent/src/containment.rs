//! Content-free nested process-containment evidence carried from a first-party
//! agent to its worker.
//!
//! Assignment identity is deliberately absent. The worker binds a lifecycle
//! endpoint to one job attempt and stamps worker/job/attempt identity only after
//! this payload has passed the bounds below.

use serde::{Deserialize, Serialize};

pub const MAX_AGENT_CONTAINMENT_IDENTIFIER_BYTES: usize = 256;
pub const MAX_AGENT_CONTAINMENT_ROOT_BYTES: usize = 512;
pub const MAX_AGENT_CONTAINMENT_REASON_BYTES: usize = 512;
pub const MAX_AGENT_CONTAINMENT_EXECUTABLE_BYTES: usize = 256;
pub const MAX_AGENT_CONTAINMENT_SURVIVORS: usize = 16;
pub const MAX_AGENT_CONTAINMENT_SIGNAL_ATTEMPTS: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AgentContainmentEventV1 {
    CleanupBlocked(AgentContainmentCleanupBlockedV1),
    CleanupCompleted(AgentContainmentCleanupCompletedV1),
    FallbackActivated(AgentContainmentFallbackV1),
}

impl AgentContainmentEventV1 {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::CleanupBlocked(event) => event.validate(),
            Self::CleanupCompleted(event) => event.validate(),
            Self::FallbackActivated(event) => event.validate(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentContainmentOwnerV1 {
    pub owner_kind: String,
    pub tool_command_id: String,
    pub backend: AgentContainmentBackendV1,
    pub root: String,
    /// Spawned boundary PID, when the sender can observe one. The default keeps
    /// lifecycle frames from older agents additively compatible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_pid: Option<u32>,
}

impl AgentContainmentOwnerV1 {
    fn validate(&self) -> Result<(), String> {
        safe_text(
            "containment owner kind",
            &self.owner_kind,
            MAX_AGENT_CONTAINMENT_IDENTIFIER_BYTES,
        )?;
        safe_text(
            "containment tool/command id",
            &self.tool_command_id,
            MAX_AGENT_CONTAINMENT_IDENTIFIER_BYTES,
        )?;
        safe_text(
            "containment root",
            &self.root,
            MAX_AGENT_CONTAINMENT_ROOT_BYTES,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentContainmentBackendV1 {
    NoProcess,
    LinuxCgroupV2,
    LinuxSupervisor,
    WindowsJob,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentContainmentProcessV1 {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    pub session_id: u32,
    pub start_time: u64,
    pub executable: String,
}

impl AgentContainmentProcessV1 {
    fn validate(&self) -> Result<(), String> {
        safe_text(
            "containment executable",
            &self.executable,
            MAX_AGENT_CONTAINMENT_EXECUTABLE_BYTES,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentContainmentSignalAttemptV1 {
    pub process: AgentContainmentProcessV1,
    pub outcome: AgentContainmentSignalOutcomeV1,
}

impl AgentContainmentSignalAttemptV1 {
    fn validate(&self) -> Result<(), String> {
        self.process.validate()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentContainmentSignalOutcomeV1 {
    Succeeded,
    ProcessGone,
    PidReused,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentContainmentTriggerV1 {
    NormalRootExit,
    Timeout,
    Cancellation,
    OwnerDrop,
    Watchdog,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentContainmentPhaseV1 {
    Discover,
    Term,
    Grace,
    Kill,
    Reap,
    VerifyEmpty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentContainmentDispositionV1 {
    AlreadyEmpty,
    Terminated,
    Killed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentContainmentReapStatusV1 {
    NotSpawned,
    Pending,
    Reaped,
    AlreadyReaped,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentContainmentCleanupBlockedV1 {
    pub owner: AgentContainmentOwnerV1,
    pub trigger: AgentContainmentTriggerV1,
    pub phase: AgentContainmentPhaseV1,
    pub repeated_failures: u64,
    pub term_attempts: Vec<AgentContainmentSignalAttemptV1>,
    pub omitted_term_attempts: u64,
    pub kill_attempts: Vec<AgentContainmentSignalAttemptV1>,
    pub omitted_kill_attempts: u64,
    pub survivors: Vec<AgentContainmentProcessV1>,
    pub omitted_survivors: u64,
}

impl AgentContainmentCleanupBlockedV1 {
    fn validate(&self) -> Result<(), String> {
        self.owner.validate()?;
        if self.repeated_failures == 0 {
            return Err("containment repeated_failures must be non-zero".to_string());
        }
        signal_attempts("TERM", &self.term_attempts)?;
        signal_attempts("KILL", &self.kill_attempts)?;
        processes(&self.survivors)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentContainmentCleanupCompletedV1 {
    pub owner: AgentContainmentOwnerV1,
    pub trigger: AgentContainmentTriggerV1,
    pub disposition: AgentContainmentDispositionV1,
    pub term_attempts: Vec<AgentContainmentSignalAttemptV1>,
    pub omitted_term_attempts: u64,
    pub kill_attempts: Vec<AgentContainmentSignalAttemptV1>,
    pub omitted_kill_attempts: u64,
    pub direct_child_reap: AgentContainmentReapStatusV1,
    pub direct_child_pid: u32,
    pub direct_child_exit_code: i64,
    pub recursive_empty_inspections: u64,
    pub survivors: Vec<AgentContainmentProcessV1>,
    pub omitted_survivors: u64,
    pub recovered_inspection_failures: u64,
    pub omitted_inspection_failures: u64,
}

impl AgentContainmentCleanupCompletedV1 {
    fn validate(&self) -> Result<(), String> {
        self.owner.validate()?;
        if self.direct_child_reap == AgentContainmentReapStatusV1::Pending {
            return Err(
                "completed containment evidence cannot have a pending child reap".to_string(),
            );
        }
        signal_attempts("TERM", &self.term_attempts)?;
        signal_attempts("KILL", &self.kill_attempts)?;
        processes(&self.survivors)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentContainmentFallbackV1 {
    pub owner: AgentContainmentOwnerV1,
    pub reason: String,
}

impl AgentContainmentFallbackV1 {
    fn validate(&self) -> Result<(), String> {
        self.owner.validate()?;
        safe_text(
            "containment fallback reason",
            &self.reason,
            MAX_AGENT_CONTAINMENT_REASON_BYTES,
        )
    }
}

fn signal_attempts(
    signal: &str,
    attempts: &[AgentContainmentSignalAttemptV1],
) -> Result<(), String> {
    if attempts.len() > MAX_AGENT_CONTAINMENT_SIGNAL_ATTEMPTS {
        return Err(format!(
            "containment {signal} attempts exceed {MAX_AGENT_CONTAINMENT_SIGNAL_ATTEMPTS}"
        ));
    }
    for attempt in attempts {
        attempt.validate()?;
    }
    Ok(())
}

fn processes(processes: &[AgentContainmentProcessV1]) -> Result<(), String> {
    if processes.len() > MAX_AGENT_CONTAINMENT_SURVIVORS {
        return Err(format!(
            "containment survivors exceed {MAX_AGENT_CONTAINMENT_SURVIVORS}"
        ));
    }
    for process in processes {
        process.validate()?;
    }
    Ok(())
}

fn safe_text(field: &str, value: &str, maximum: usize) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{field} must not be empty"));
    }
    if value.len() > maximum {
        return Err(format!("{field} exceeds {maximum} bytes"));
    }
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
        return Err(format!(
            "{field} contains forbidden credential-like content"
        ));
    }
    Ok(())
}
