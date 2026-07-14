// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};
use temper_protocol_activity::{
    AgentActivityAcknowledgement, AgentActivityBatch, AgentActivityCapturePolicyV1,
};

/// Authenticated worker envelope for one durable agent-activity forwarding batch.
///
/// The canonical events remain owned by `temper-protocol-activity`; this wrapper
/// carries only transport identity and the immutable assignment-attempt binding
/// required by the engine journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerActivityBatch {
    pub protocol_version: u32,
    pub worker_id: String,
    pub assignment_id: String,
    pub capture_policy: AgentActivityCapturePolicyV1,
    pub batch: AgentActivityBatch,
}

/// Worker-protocol acknowledgement for activity that the engine has durably
/// appended. A worker may compact only through the enclosed contiguous cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerActivityAcknowledgement {
    pub protocol_version: u32,
    pub worker_id: String,
    pub acknowledgement: AgentActivityAcknowledgement,
}
