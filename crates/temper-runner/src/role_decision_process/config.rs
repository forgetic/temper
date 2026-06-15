//! Configuration for invoking an out-of-process workflow-role decision engine.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use super::error::WorkflowRoleDecisionProcessError;

/// Configuration for invoking an out-of-process workflow-role decision engine.
///
/// `Debug` redacts forwarded environment values (which may be secrets such as
/// API keys or Forge tokens) while keeping their names visible.
#[derive(Clone, Eq, PartialEq)]
pub struct WorkflowRoleDecisionProcessConfig {
    /// Program to execute.
    pub program: PathBuf,
    /// Arguments supplied to the program.
    pub args: Vec<String>,
    /// Optional working directory for the process.
    pub working_dir: Option<PathBuf>,
    /// Environment variables forwarded to the subprocess as resolved
    /// name→value pairs. Values are resolved once at the config/arg boundary;
    /// this adapter never reads ambient process environment at spawn time.
    pub env: Vec<(String, String)>,
    /// Maximum wall-clock duration for one decision.
    pub timeout: Duration,
}

impl fmt::Debug for WorkflowRoleDecisionProcessConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let env_redacted: Vec<(&str, &str)> = self
            .env
            .iter()
            .map(|(name, _value)| (name.as_str(), "<redacted>"))
            .collect();
        formatter
            .debug_struct("WorkflowRoleDecisionProcessConfig")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("working_dir", &self.working_dir)
            .field("env", &env_redacted)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl WorkflowRoleDecisionProcessConfig {
    /// Default one-decision timeout.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

    /// Builds process configuration with no inherited environment and the
    /// default timeout.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            working_dir: None,
            env: Vec::new(),
            timeout: Self::DEFAULT_TIMEOUT,
        }
    }

    /// Replaces command arguments.
    pub fn with_args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the working directory.
    pub fn with_working_dir(mut self, working_dir: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(working_dir.into());
        self
    }

    /// Replaces the forwarded environment with resolved name→value pairs.
    ///
    /// Values must be resolved by the caller at the config/arg boundary (where
    /// process environment is read once); this adapter forwards them verbatim
    /// and never reads ambient environment itself.
    pub fn with_env<I, K, V>(mut self, pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.env = pairs
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect();
        self
    }

    /// Sets the timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

pub(super) fn validate_config(
    config: &WorkflowRoleDecisionProcessConfig,
) -> Result<(), WorkflowRoleDecisionProcessError> {
    if config.program.as_os_str().is_empty() {
        return Err(WorkflowRoleDecisionProcessError::InvalidConfig {
            field: "role_decision_process.program",
            message: "must not be empty".to_string(),
        });
    }
    if config.timeout.is_zero() {
        return Err(WorkflowRoleDecisionProcessError::InvalidConfig {
            field: "role_decision_process.timeout",
            message: "must be greater than zero".to_string(),
        });
    }
    for (name, _value) in &config.env {
        validate_env_name(name)?;
    }
    Ok(())
}

fn validate_env_name(name: &str) -> Result<(), WorkflowRoleDecisionProcessError> {
    if name.is_empty() || name.contains('=') || name.contains('\0') {
        return Err(WorkflowRoleDecisionProcessError::InvalidConfig {
            field: "role_decision_process.env_allowlist",
            message: format!("invalid environment variable name `{name}`"),
        });
    }
    Ok(())
}
