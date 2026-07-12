//! DTOs for the worker-owned, read-only Forge context side channel.

use serde::{Deserialize, Serialize};

use crate::{ForgeContextErrorCode, ForgeContextOperation, ForgeContextResult};

/// CLI flag carrying the per-run worker loopback endpoint for Forge reads.
pub const FORGE_CONTEXT_ADDRESS_FLAG: &str = "--forge-context-address";

/// One untrusted operation emitted by the child agent.
///
/// Assignment identity and credentials are deliberately absent: the worker
/// binds those to the listener created for the active run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeContextRequest {
    pub protocol_version: u32,
    pub operation: ForgeContextOperation,
}

/// Bounded response returned by the worker to the same tool call.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForgeContextResponse {
    pub protocol_version: u32,
    #[serde(flatten)]
    pub outcome: ForgeContextToolOutcome,
}

impl ForgeContextResponse {
    pub fn success(result: ForgeContextResult) -> Self {
        Self {
            protocol_version: crate::PROTOCOL_VERSION,
            outcome: ForgeContextToolOutcome::Success { result },
        }
    }

    pub fn error(code: ForgeContextErrorCode) -> Self {
        Self {
            protocol_version: crate::PROTOCOL_VERSION,
            outcome: ForgeContextToolOutcome::Error { code },
        }
    }
}

/// Exactly one successful result or one stable public error code.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ForgeContextToolOutcome {
    Success { result: ForgeContextResult },
    Error { code: ForgeContextErrorCode },
}
