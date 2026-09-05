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

/// Closed policy reasons that may authoritatively classify a first-party stop.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPolicyFailureReasonV1 {
    DecisionAnchorRecoveryExhausted,
}

impl AgentPolicyFailureReasonV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DecisionAnchorRecoveryExhausted => "decision_anchor_recovery_exhausted",
        }
    }
}

/// Provider-neutral policy terminal produced from the coding agent's typed
/// state-machine stop, never from model or process text.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPolicyFailureV1 {
    pub reason: AgentPolicyFailureReasonV1,
}

impl AgentPolicyFailureV1 {
    pub const fn decision_anchor_recovery_exhausted() -> Self {
        Self {
            reason: AgentPolicyFailureReasonV1::DecisionAnchorRecoveryExhausted,
        }
    }
}

/// A terminal diagnostic written when a first-party agent has no
/// [`crate::WorkspaceResult`] to return.
///
/// The closed shape intentionally carries no generic text, stderr, prompt,
/// model response, or credential field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTerminalOutputV1 {
    pub protocol_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_failure: Option<ModelFailureV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_failure: Option<AgentPolicyFailureV1>,
}

impl AgentTerminalOutputV1 {
    /// Builds a canonical terminal output from an authoritative first-party
    /// model diagnostic.
    pub fn model_failure(mut model_failure: ModelFailureV1) -> Self {
        model_failure.normalize();
        Self {
            protocol_version: AGENT_TERMINAL_PROTOCOL_VERSION,
            model_failure: Some(model_failure),
            policy_failure: None,
        }
    }

    /// Builds the only policy failure admitted by the v1 terminal protocol.
    pub const fn decision_anchor_recovery_exhausted() -> Self {
        Self {
            protocol_version: AGENT_TERMINAL_PROTOCOL_VERSION,
            model_failure: None,
            policy_failure: Some(AgentPolicyFailureV1::decision_anchor_recovery_exhausted()),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.protocol_version != AGENT_TERMINAL_PROTOCOL_VERSION {
            return Err(format!(
                "unsupported terminal protocol version {}",
                self.protocol_version
            ));
        }
        match (&self.model_failure, self.policy_failure) {
            (Some(model_failure), None) => {
                model_failure.validate().map_err(|error| error.to_string())
            }
            (None, Some(_)) => Ok(()),
            (None, None) => {
                Err("terminal output must contain exactly one typed failure".to_string())
            }
            (Some(_), Some(_)) => {
                Err("terminal output contains ambiguous typed failures".to_string())
            }
        }
    }
}
