use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use temper_protocol_context::W3cTraceContext;

use crate::{ACTIVITY_PROTOCOL_VERSION, ActivityValidationError};

mod failure;
mod model_call;
mod prompt;
mod terminal;
pub use failure::*;
pub use model_call::*;
pub use prompt::*;
pub use terminal::*;

/// Identity assigned by the worker and held constant for an entire run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAssignmentIdentityV1 {
    pub job_id: String,
    pub repository: String,
    pub artifact_ref: String,
    pub role: String,
    pub action: String,
    pub correlation_key: String,
    /// Optional assignment-delivery context. It is not reused as workstream
    /// identity across later runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_context: Option<W3cTraceContext>,
}

/// Scope identity supplied by the agent, independently of a display label.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentScopeV1 {
    pub id: String,
    pub kind: AgentScopeKindV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentScopeKindV1 {
    Main,
    SubAgent,
}

/// An untrusted child-to-worker frame.
///
/// Trusted run and assignment fields are intentionally absent. Unknown fields
/// are rejected, so a child cannot smuggle those fields into this envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentActivityFrameV1 {
    pub version: u32,
    /// RFC3339 source occurrence time.
    pub occurred_at: String,
    /// Monotonic milliseconds elapsed since the child's run origin.
    pub elapsed_ms: u64,
    pub scope: AgentScopeV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    pub event: AgentActivityEventV1,
}

impl AgentActivityFrameV1 {
    pub fn validate(&self) -> Result<(), ActivityValidationError> {
        crate::validate_frame(self)
    }
}

/// An attachment-bearing child-to-worker record.
///
/// Producers keep bare [`AgentActivityFrameV1`] values on the wire when no
/// attachment is needed. This envelope is reserved for a frame whose content
/// references are transported in `blobs` as one atomic queue item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentActivityChildRecordV1 {
    pub frame: AgentActivityFrameV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blobs: Vec<BlobAttachmentV1>,
}

impl AgentActivityChildRecordV1 {
    pub fn validate(&self) -> Result<(), ActivityValidationError> {
        crate::validate_child_record(self)
    }
}

/// Canonical, worker-stamped activity event.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRunEventV1 {
    pub version: u32,
    pub run_id: String,
    /// Run-local sequence number. The first event in a complete run is 1.
    pub seq: u64,
    /// RFC3339 occurrence time.
    pub occurred_at: String,
    /// Monotonic milliseconds elapsed since the worker's run origin.
    pub elapsed_ms: u64,
    pub assignment: AgentAssignmentIdentityV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    pub scope: AgentScopeV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    pub event: AgentActivityEventV1,
}

impl AgentRunEventV1 {
    pub fn validate(&self) -> Result<(), ActivityValidationError> {
        crate::validate_run_event(self)
    }
}

/// At-least-once forwarding unit. Events must be contiguous and attachments
/// must exactly cover the blob references in this batch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentActivityBatch {
    pub version: u32,
    pub run_id: String,
    pub first_seq: u64,
    pub events: Vec<AgentRunEventV1>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blobs: Vec<BlobAttachmentV1>,
}

impl AgentActivityBatch {
    pub fn validate(&self) -> Result<(), ActivityValidationError> {
        crate::validate_batch(self)
    }
}

/// Highest durably accepted contiguous run sequence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentActivityAcknowledgement {
    pub version: u32,
    pub run_id: String,
    pub highest_contiguous_seq: u64,
}

impl AgentActivityAcknowledgement {
    pub fn validate(&self) -> Result<(), ActivityValidationError> {
        crate::validate_acknowledgement(self)
    }
}

/// Concise alias used by transport implementations.
pub type AgentActivityAck = AgentActivityAcknowledgement;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureModeV1 {
    Off,
    Metadata,
    Transcript,
    Diagnostic,
}

/// Capture policy transported between independently deployed tiers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentActivityCapturePolicyV1 {
    pub version: u32,
    pub capture: CaptureModeV1,
    pub retention_days: u32,
    pub max_run_bytes: u64,
    pub max_inline_bytes: u32,
    pub max_blob_bytes: u64,
    pub capture_thinking: bool,
}

impl AgentActivityCapturePolicyV1 {
    pub fn validate(&self) -> Result<(), ActivityValidationError> {
        crate::validate_capture_policy(self)
    }
}

impl Default for AgentActivityCapturePolicyV1 {
    fn default() -> Self {
        Self {
            version: ACTIVITY_PROTOCOL_VERSION,
            capture: CaptureModeV1::Metadata,
            retention_days: 14,
            max_run_bytes: 50_000_000,
            max_inline_bytes: crate::MAX_INLINE_CONTENT_BYTES as u32,
            max_blob_bytes: crate::MAX_BLOB_ATTACHMENT_BYTES as u64,
            capture_thinking: false,
        }
    }
}

/// A hard-bounded inline captured value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InlineContentV1 {
    pub text: String,
    pub truncated: bool,
}

/// Captured content is either bounded inline text or a content-addressed blob;
/// there is no arbitrary JSON extension object.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "storage", rename_all = "snake_case", deny_unknown_fields)]
pub enum CapturedContentV1 {
    Inline(InlineContentV1),
    Blob { blob: BlobReferenceV1 },
}

impl CapturedContentV1 {
    pub(crate) fn blob_reference(&self) -> Option<&BlobReferenceV1> {
        match self {
            Self::Inline(_) => None,
            Self::Blob { blob } => Some(blob),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlobMediaTypeV1 {
    TextPlainUtf8,
    TextMarkdownUtf8,
    ApplicationJson,
}

/// Content-addressed reference to a bounded transcript blob.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlobReferenceV1 {
    /// Lowercase `sha256:<64 hex characters>` digest.
    pub digest: String,
    pub bytes: u64,
    pub media_type: BlobMediaTypeV1,
}

impl BlobReferenceV1 {
    pub fn for_bytes(media_type: BlobMediaTypeV1, bytes: &[u8]) -> Self {
        Self {
            digest: sha256_digest(bytes),
            bytes: bytes.len() as u64,
            media_type,
        }
    }

    pub fn validate(&self) -> Result<(), ActivityValidationError> {
        crate::validate_blob_reference(self)
    }
}

/// Inline attachment used to transport a referenced blob with a batch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlobAttachmentV1 {
    pub blob: BlobReferenceV1,
    pub data_base64: String,
}

impl BlobAttachmentV1 {
    pub fn from_bytes(media_type: BlobMediaTypeV1, bytes: &[u8]) -> Self {
        Self {
            blob: BlobReferenceV1::for_bytes(media_type, bytes),
            data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    pub fn decode(&self) -> Result<Vec<u8>, ActivityValidationError> {
        crate::validation::decode_attachment(self)
    }

    pub fn validate(&self) -> Result<(), ActivityValidationError> {
        crate::validate_blob_attachment(self)
    }
}

pub type BlobReference = BlobReferenceV1;
pub type BlobAttachment = BlobAttachmentV1;

fn sha256_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut rendered = String::with_capacity(7 + digest.len() * 2);
    rendered.push_str("sha256:");
    for byte in digest {
        write!(&mut rendered, "{byte:02x}").expect("writing to a String cannot fail");
    }
    rendered
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", deny_unknown_fields)]
pub enum AgentActivityEventV1 {
    #[serde(rename = "run.started")]
    RunStarted(RunStartedV1),
    #[serde(rename = "run.finished")]
    RunFinished(RunFinishedV1),
    #[serde(rename = "scope.started")]
    ScopeStarted(ScopeStartedV1),
    #[serde(rename = "scope.finished")]
    ScopeFinished(ScopeFinishedV1),
    #[serde(rename = "prompt.prepared")]
    PromptPrepared(PromptPreparedV1),
    #[serde(rename = "turn.started")]
    TurnStarted(TurnStartedV1),
    #[serde(rename = "turn.finished")]
    TurnFinished(TurnFinishedV1),
    #[serde(rename = "model.call.started")]
    ModelCallStarted(ModelCallStartedV1),
    #[serde(rename = "model.call.retrying")]
    ModelCallRetrying(ModelCallRetryingV1),
    #[serde(rename = "model.call.finished")]
    ModelCallFinished(ModelCallFinishedV1),
    #[serde(rename = "assistant.message")]
    AssistantMessage(AssistantMessageV1),
    #[serde(rename = "output.text.delta")]
    OutputTextDelta(OutputDeltaV1),
    #[serde(rename = "output.thinking.delta")]
    OutputThinkingDelta(OutputDeltaV1),
    #[serde(rename = "tool.started")]
    ToolStarted(ToolStartedV1),
    #[serde(rename = "tool.finished")]
    ToolFinished(ToolFinishedV1),
    #[serde(rename = "steering.applied")]
    SteeringApplied(SteeringAppliedV1),
    #[serde(rename = "usage")]
    Usage(UsageV1),
    #[serde(rename = "trace.gap")]
    TraceGap(TraceGapV1),
    #[serde(rename = "run.failed")]
    RunFailed(RunFailedV1),
}

impl AgentActivityEventV1 {
    pub const fn event_type(&self) -> &'static str {
        match self {
            Self::RunStarted(_) => "run.started",
            Self::RunFinished(_) => "run.finished",
            Self::ScopeStarted(_) => "scope.started",
            Self::ScopeFinished(_) => "scope.finished",
            Self::PromptPrepared(_) => "prompt.prepared",
            Self::TurnStarted(_) => "turn.started",
            Self::TurnFinished(_) => "turn.finished",
            Self::ModelCallStarted(_) => "model.call.started",
            Self::ModelCallRetrying(_) => "model.call.retrying",
            Self::ModelCallFinished(_) => "model.call.finished",
            Self::AssistantMessage(_) => "assistant.message",
            Self::OutputTextDelta(_) => "output.text.delta",
            Self::OutputThinkingDelta(_) => "output.thinking.delta",
            Self::ToolStarted(_) => "tool.started",
            Self::ToolFinished(_) => "tool.finished",
            Self::SteeringApplied(_) => "steering.applied",
            Self::Usage(_) => "usage",
            Self::TraceGap(_) => "trace.gap",
            Self::RunFailed(_) => "run.failed",
        }
    }

    pub const fn classification(&self) -> EventClassificationV1 {
        match self {
            Self::RunStarted(_) => EventClassificationV1::Run,
            Self::RunFinished(_) => EventClassificationV1::Terminal,
            Self::ScopeStarted(_) | Self::ScopeFinished(_) => EventClassificationV1::Scope,
            Self::PromptPrepared(_) => EventClassificationV1::Prompt,
            Self::TurnStarted(_) | Self::TurnFinished(_) => EventClassificationV1::Turn,
            Self::ModelCallStarted(_) | Self::ModelCallFinished(_) => {
                EventClassificationV1::ModelCall
            }
            Self::ModelCallRetrying(_) => EventClassificationV1::Retry,
            Self::AssistantMessage(_) => EventClassificationV1::AssistantMessage,
            Self::OutputTextDelta(_) | Self::OutputThinkingDelta(_) => EventClassificationV1::Delta,
            Self::ToolStarted(_) | Self::ToolFinished(_) => EventClassificationV1::Tool,
            Self::SteeringApplied(_) => EventClassificationV1::Steering,
            Self::Usage(_) => EventClassificationV1::Usage,
            Self::TraceGap(_) => EventClassificationV1::Gap,
            Self::RunFailed(_) => EventClassificationV1::Error,
        }
    }

    pub const fn priority(&self) -> EventPriorityV1 {
        match self {
            Self::OutputTextDelta(_) | Self::OutputThinkingDelta(_) => EventPriorityV1::Droppable,
            Self::AssistantMessage(_) | Self::SteeringApplied(_) => EventPriorityV1::Normal,
            _ => EventPriorityV1::Required,
        }
    }

    pub const fn is_droppable(&self) -> bool {
        matches!(self.priority(), EventPriorityV1::Droppable)
    }

    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::RunFinished(_) | Self::RunFailed(_))
    }

    pub const fn is_boundary(&self) -> bool {
        matches!(self.priority(), EventPriorityV1::Required)
    }

    /// Replaces untrusted provider/model retry diagnostics with the sole
    /// allowlisted canonical summary. Failure code and retryability are typed
    /// facts and intentionally remain unchanged.
    pub fn sanitize_retry_failure_message(&mut self) {
        if let Self::ModelCallRetrying(retry) = self {
            retry.failure.message = crate::MODEL_CALL_RETRY_FAILURE_MESSAGE.to_string();
        }
    }

    /// Applies the shared fail-closed normalization to a finished model call.
    ///
    /// Trust boundaries use this before validation or serialization. Besides
    /// sanitizing supplied detail, it canonicalizes the legacy
    /// `succeeded + stop_reason=error` shape and supplies explicit redacted
    /// detail for newly ingested failed calls. Retained records remain readable
    /// because ordinary deserialization and validation do not call this method.
    pub fn normalize_model_failure(&mut self) {
        let Self::ModelCallFinished(finished) = self else {
            return;
        };
        if finished.status == ModelCallStatusV1::Succeeded
            && finished.stop_reason == Some(StopReasonV1::Error)
        {
            finished.status = ModelCallStatusV1::Failed;
        }
        if finished.status == ModelCallStatusV1::Failed && finished.failure.is_none() {
            finished.failure = Some(ModelFailureV1::redacted_unknown(
                "unknown", "unknown", false,
            ));
        }
        if let Some(failure) = &mut finished.failure {
            failure.normalize();
        }
    }

    pub(crate) const fn is_host_only(&self) -> bool {
        matches!(
            self,
            Self::RunStarted(_) | Self::RunFinished(_) | Self::RunFailed(_)
        )
    }

    pub(crate) fn content_references(&self) -> Vec<&BlobReferenceV1> {
        let content = match self {
            Self::PromptPrepared(data) => data.content.as_ref(),
            Self::AssistantMessage(data) => Some(&data.content),
            Self::ToolStarted(data) => data.arguments.as_ref(),
            Self::ToolFinished(data) => data.result.as_ref(),
            Self::SteeringApplied(data) => data.instruction.as_ref(),
            _ => None,
        };
        content
            .and_then(CapturedContentV1::blob_reference)
            .into_iter()
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventClassificationV1 {
    Run,
    Scope,
    Prompt,
    Turn,
    ModelCall,
    AssistantMessage,
    Delta,
    Tool,
    Steering,
    Retry,
    Usage,
    Gap,
    Error,
    Terminal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventPriorityV1 {
    Required,
    Normal,
    Droppable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunStartedV1 {
    pub capture: CaptureModeV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunFinishedV1 {
    pub status: RunStatusV1,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReasonV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatusV1 {
    Succeeded,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeStartedV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeFinishedV1 {
    pub status: ScopeStatusV1,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<AgentTerminalReasonV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeStatusV1 {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnStartedV1 {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnFinishedV1 {
    pub duration_ms: u64,
    pub stop_reason: StopReasonV1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReasonV1 {
    EndTurn,
    ToolUse,
    MaxTokens,
    Cancelled,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantMessageV1 {
    pub message_id: String,
    pub content: CapturedContentV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputDeltaV1 {
    pub delta: InlineContentV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolStartedV1 {
    pub call_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<CapturedContentV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolFinishedV1 {
    pub call_id: String,
    pub name: String,
    pub status: ToolStatusV1,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<CapturedContentV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatusV1 {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteeringAppliedV1 {
    pub source: SteeringSourceV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction: Option<CapturedContentV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteeringSourceV1 {
    User,
    Worker,
    Policy,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailureInfoV1 {
    pub code: FailureCodeV1,
    pub message: String,
    pub retryable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCodeV1 {
    Provider,
    Timeout,
    Tool,
    ChildProcess,
    Cancelled,
    Policy,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunFailedV1 {
    pub failure: FailureInfoV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageV1 {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceGapV1 {
    pub dropped_events: u64,
    pub dropped_bytes: u64,
    pub kinds: Vec<DroppedEventKindV1>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DroppedEventKindV1 {
    TextDelta,
    ThinkingDelta,
}
