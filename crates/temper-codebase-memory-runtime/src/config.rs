use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub(crate) const HELPER_MODE: &str = "--temper-codebase-memory-bootstrap";
pub(crate) const MAX_CONTROL_BYTES: usize = 65_536;

/// Values resolved from operator configuration by the parent composition root.
/// This is never accepted as an agent-originated launch request.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchConfig {
    pub command: String,
    pub args: Vec<String>,
    pub workspace: PathBuf,
    #[serde(default)]
    pub environment: Vec<(String, String)>,
    pub startup_timeout: Duration,
    pub admission_timeout: Duration,
}

impl LaunchConfig {
    pub(crate) fn bootstrap_args(&self) -> Vec<String> {
        let mut args = self.args.iter();
        let mut result = Vec::new();
        while let Some(argument) = args.next() {
            if argument == "--tool-profile" {
                args.next();
            } else if !argument.starts_with("--tool-profile=") {
                result.push(argument.clone());
            }
        }
        result.push("--tool-profile=analysis".into());
        result
    }

    pub(crate) fn validate(&self) -> Result<(), ProviderBootstrapFailure> {
        if self.command.trim().is_empty()
            || !self.workspace.is_absolute()
            || self.startup_timeout.is_zero()
            || self.admission_timeout.is_zero()
            || self.startup_timeout > Duration::from_secs(3_600)
            || self.admission_timeout > Duration::from_secs(86_400)
        {
            return Err(ProviderBootstrapFailure::Configuration);
        }
        Ok(())
    }
}

/// Closed diagnostic categories: never carry provider output, arguments or paths.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderBootstrapFailure {
    Configuration,
    UnsupportedPlatform,
    Spawn,
    Timeout,
    Transport,
    ProviderContract,
}

impl std::fmt::Display for ProviderBootstrapFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "shared provider bootstrap: {self:?}")
    }
}

impl std::error::Error for ProviderBootstrapFailure {}

#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum BootstrapReply {
    Admitted,
    Failed { category: ProviderBootstrapFailure },
}
