// SPDX-License-Identifier: MPL-2.0

//! Private first-party terminal failure carrier.

use serde::{Deserialize, Serialize};
use temper_protocol_activity::ModelFailureV1;

/// First-party-only flag naming the private terminal output file.
pub const TERMINAL_OUTPUT_FLAG: &str = "--terminal-output";
/// Version of the bounded terminal output document.
pub const AGENT_TERMINAL_PROTOCOL_VERSION: u32 = 1;
/// Hard bound for the complete first-party terminal output JSON document.
pub const MAX_AGENT_TERMINAL_OUTPUT_BYTES: usize = 4096;

/// A terminal diagnostic written when a first-party agent has no
/// [`crate::WorkspaceResult`] to return.
///
/// The closed shape intentionally carries no generic text, stderr, prompt,
/// model response, or credential field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTerminalOutputV1 {
    pub protocol_version: u32,
    pub model_failure: ModelFailureV1,
}

impl AgentTerminalOutputV1 {
    /// Builds a canonical terminal output from an authoritative first-party
    /// model diagnostic.
    pub fn model_failure(mut model_failure: ModelFailureV1) -> Self {
        model_failure.normalize();
        Self {
            protocol_version: AGENT_TERMINAL_PROTOCOL_VERSION,
            model_failure,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.protocol_version != AGENT_TERMINAL_PROTOCOL_VERSION {
            return Err(format!(
                "unsupported terminal protocol version {}",
                self.protocol_version
            ));
        }
        self.model_failure
            .validate()
            .map_err(|error| error.to_string())
    }
}
