use serde::{Deserialize, Serialize};

use super::{FailureInfoV1, ModelFailureV1, StopReasonV1};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCallStartedV1 {
    pub call_id: String,
    pub provider: String,
    pub model: String,
    pub attempt: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCallRetryingV1 {
    pub call_id: String,
    pub next_attempt: u32,
    pub delay_ms: u64,
    pub failure: FailureInfoV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCallFinishedV1 {
    pub call_id: String,
    pub attempt: u32,
    pub status: ModelCallStatusV1,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_to_first_token_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReasonV1>,
    /// Safe structured detail for newly produced failed calls. Retained V1
    /// records written before this field was introduced omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<ModelFailureV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCallStatusV1 {
    Succeeded,
    Failed,
    Cancelled,
}
