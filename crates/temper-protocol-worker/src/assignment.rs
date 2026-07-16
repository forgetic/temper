// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};
use serde_json::Value;
use temper_protocol_context::W3cTraceContext;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    pub role: String,
    pub repo: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capacity {
    pub max_concurrent_jobs: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Register {
    pub protocol_version: u32,
    pub worker_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_pool: Option<String>,
    pub capabilities: Vec<Capability>,
    pub capacity: Capacity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Poll {
    pub protocol_version: u32,
    pub worker_id: String,
    pub free_capacity: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wait_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    /// String or numeric artifact identity, preserved as JSON per the protocol.
    pub item: Value,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assign {
    pub protocol_version: u32,
    /// Optional W3C context copied from the typed job payload so transports do
    /// not need to inspect opaque assignment JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_context: Option<W3cTraceContext>,
    pub job_id: String,
    /// Opaque daemon-generated fence for this particular dispatch of `job_id`.
    /// Optional only so legacy protocol fixtures remain readable; a current
    /// worker refuses assignments that omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<String>,
    pub role: String,
    /// Primary repository path (`owner/name`) -- the home of the coordinating
    /// artifact. The full repo set to assemble travels in the job payload's
    /// [`crate::WorkspaceManifest`].
    pub repo: String,
    pub artifact: Artifact,
    pub job_payload: Value,
}
