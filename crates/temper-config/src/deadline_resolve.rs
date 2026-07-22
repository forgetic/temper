// SPDX-License-Identifier: MPL-2.0

//! Resolution and cross-field validation for agent and worker liveness limits.

use std::time::Duration;

use crate::error::ConfigError;
use crate::resolved::{
    AgentOperationLimits, AgentSettings, STANDALONE_FINAL_KILL_ALLOWANCE,
    STANDALONE_HTTP_DRAIN_ALLOWANCE, WorkerLivenessLimits,
};
use crate::schema::{AgentDeadlineConfig, Config};

const DEFAULT_MAX_NO_PROGRESS_SECS: u64 = 900;
const DEFAULT_MAX_RUN_SECS: Option<u64> = None;
const DEFAULT_GRACEFUL_CANCELLATION_GRACE_SECS: u64 = 10;
const DEFAULT_FORCED_TERMINATION_GRACE_SECS: u64 = 5;
pub(crate) const DEFAULT_STANDALONE_SHUTDOWN_BUDGET_SECS: u64 = 30;
const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 600;
const DEFAULT_MODEL_CONNECT_TIMEOUT_SECS: u64 = 120;
const DEFAULT_MODEL_IDLE_TIMEOUT_SECS: u64 = 120;

pub(crate) fn resolve_worker_liveness_limits(
    config: &Config,
) -> Result<WorkerLivenessLimits, ConfigError> {
    let max_no_progress = positive_duration_secs(
        config
            .worker
            .max_no_progress_secs
            .unwrap_or(DEFAULT_MAX_NO_PROGRESS_SECS),
        "worker.max_no_progress_secs",
    )?;
    let max_run_secs = config.worker.max_run_secs.or(DEFAULT_MAX_RUN_SECS);
    let max_run = max_run_secs
        .map(|secs| positive_duration_secs(secs, "worker.max_run_secs"))
        .transpose()?;
    let graceful_cancellation_grace = positive_duration_secs(
        config
            .worker
            .graceful_cancellation_grace_secs
            .unwrap_or(DEFAULT_GRACEFUL_CANCELLATION_GRACE_SECS),
        "worker.graceful_cancellation_grace_secs",
    )?;
    let forced_termination_grace = positive_duration_secs(
        config
            .worker
            .forced_termination_grace_secs
            .unwrap_or(DEFAULT_FORCED_TERMINATION_GRACE_SECS),
        "worker.forced_termination_grace_secs",
    )?;
    Ok(WorkerLivenessLimits {
        max_no_progress,
        max_run,
        graceful_cancellation_grace,
        forced_termination_grace,
    })
}

pub(crate) fn validate_liveness_ordering(
    heartbeat_interval: Duration,
    worker: WorkerLivenessLimits,
    agent: &AgentSettings,
) -> Result<(), ConfigError> {
    if heartbeat_interval >= worker.max_no_progress {
        return Err(ConfigError::invalid(
            "worker.heartbeat_interval_ms must be less than worker.max_no_progress_secs",
        ));
    }
    let cancellation_graces = worker
        .graceful_cancellation_grace
        .checked_add(worker.forced_termination_grace)
        .ok_or_else(|| ConfigError::invalid("worker cancellation grace periods overflow"))?;
    if cancellation_graces >= worker.max_no_progress {
        return Err(ConfigError::invalid(
            "worker graceful_cancellation_grace_secs plus forced_termination_grace_secs must be less than worker.max_no_progress_secs",
        ));
    }
    validate_operation_deadlines(
        "agent.deadlines",
        agent.operation_limits,
        worker.max_no_progress,
    )?;
    for (name, profile) in &agent.profiles {
        if profile.command.is_empty() || is_first_party_agent_command(&profile.command) {
            validate_operation_deadlines(
                &format!("agent.profiles.{name}.deadlines"),
                profile.operation_limits,
                worker.max_no_progress,
            )?;
        }
    }
    Ok(())
}

pub(crate) fn resolve_agent_operation_limits(
    raw: &AgentDeadlineConfig,
    inherited: Option<AgentOperationLimits>,
    field: &str,
) -> Result<AgentOperationLimits, ConfigError> {
    let defaults = inherited.unwrap_or(AgentOperationLimits {
        tool_timeout: Duration::from_secs(DEFAULT_TOOL_TIMEOUT_SECS),
        model_connect_timeout: Duration::from_secs(DEFAULT_MODEL_CONNECT_TIMEOUT_SECS),
        model_idle_timeout: Duration::from_secs(DEFAULT_MODEL_IDLE_TIMEOUT_SECS),
    });
    Ok(AgentOperationLimits {
        tool_timeout: raw
            .tool_timeout_secs
            .map(|secs| positive_duration_secs(secs, &format!("{field}.tool_timeout_secs")))
            .transpose()?
            .unwrap_or(defaults.tool_timeout),
        model_connect_timeout: raw
            .model_connect_timeout_secs
            .map(|secs| {
                positive_duration_secs(secs, &format!("{field}.model_connect_timeout_secs"))
            })
            .transpose()?
            .unwrap_or(defaults.model_connect_timeout),
        model_idle_timeout: raw
            .model_idle_timeout_secs
            .map(|secs| positive_duration_secs(secs, &format!("{field}.model_idle_timeout_secs")))
            .transpose()?
            .unwrap_or(defaults.model_idle_timeout),
    })
}

pub(crate) fn positive_duration_millis(millis: u64, field: &str) -> Result<Duration, ConfigError> {
    if millis == 0 {
        return Err(ConfigError::invalid(format!(
            "{field} must be greater than zero"
        )));
    }
    Ok(Duration::from_millis(millis))
}

fn validate_operation_deadlines(
    field: &str,
    limits: AgentOperationLimits,
    max_no_progress: Duration,
) -> Result<(), ConfigError> {
    for (name, duration) in [
        ("tool_timeout_secs", limits.tool_timeout),
        ("model_connect_timeout_secs", limits.model_connect_timeout),
        ("model_idle_timeout_secs", limits.model_idle_timeout),
    ] {
        if duration >= max_no_progress {
            return Err(ConfigError::invalid(format!(
                "{field}.{name} must be less than worker.max_no_progress_secs for first-party agents"
            )));
        }
    }
    Ok(())
}

fn is_first_party_agent_command(command: &[String]) -> bool {
    let Some(program) = command.first() else {
        return false;
    };
    let executable = std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    executable == "temper-agent"
        || (executable == "temper" && command.get(1).is_some_and(|arg| arg == "agent"))
}

pub(crate) fn validate_standalone_shutdown_budget(
    budget: Duration,
    worker: WorkerLivenessLimits,
) -> Result<(), ConfigError> {
    let required = worker
        .graceful_cancellation_grace
        .checked_add(worker.forced_termination_grace)
        .and_then(|graces| graces.checked_add(STANDALONE_HTTP_DRAIN_ALLOWANCE))
        .and_then(|allowances| allowances.checked_add(STANDALONE_FINAL_KILL_ALLOWANCE))
        .ok_or_else(|| ConfigError::invalid("standalone shutdown allowances overflow"))?;
    if budget <= required {
        return Err(ConfigError::invalid(
            "deployment.standalone_shutdown_budget_secs must strictly exceed worker graceful_cancellation_grace_secs plus forced_termination_grace_secs plus the 5 second HTTP-drain allowance plus the 5 second final emergency-kill allowance",
        ));
    }
    Ok(())
}

pub(crate) fn positive_duration_secs(secs: u64, field: &str) -> Result<Duration, ConfigError> {
    if secs == 0 {
        return Err(ConfigError::invalid(format!(
            "{field} must be greater than zero"
        )));
    }
    Ok(Duration::from_secs(secs))
}
