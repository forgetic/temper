use serde::{Deserialize, Serialize};

use super::CapturedContentV1;

/// The exact model-visible context prepared for the first provider call.
///
/// Field order is part of the canonical compact JSON representation produced
/// by [`PromptSnapshotV1::to_canonical_json_bytes`]. Tool order is registry
/// order and is never sorted by the protocol.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptSnapshotV1 {
    pub system_prompt: Option<String>,
    pub initial_user_message: String,
    pub tools: Vec<PromptToolDefinitionV1>,
}

impl PromptSnapshotV1 {
    /// Serializes the complete snapshot as compact deterministic JSON.
    pub fn to_canonical_json_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Serializes only the ordered tool manifest using the same canonical form.
    pub fn tools_to_canonical_json_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(&self.tools)
    }
}

/// One provider-visible tool definition in a prompt snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptToolDefinitionV1 {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Why the optional source-equivalent prompt snapshot body is present or
/// absent from a `prompt.prepared` event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCaptureDispositionV1 {
    Captured,
    OmittedPolicy,
    OmittedLimit,
    OmittedQuota,
}

/// Metadata and optional complete content for the startup prompt snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPreparedV1 {
    pub system_prompt_present: bool,
    pub system_prompt_bytes: u64,
    pub initial_user_message_bytes: u64,
    pub tool_manifest_bytes: u64,
    pub tool_count: u32,
    pub original_snapshot_bytes: u64,
    pub captured_bytes: u64,
    pub disposition: PromptCaptureDispositionV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<CapturedContentV1>,
}
