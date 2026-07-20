use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use temper_agent_core::{
    AgentEvent, AgentStop, EventSink, ModelCallStatus, StreamDelta, ToolCallStatus,
};
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityCapturePolicyV1, AgentActivityChildRecordV1,
    AgentActivityEventV1, AgentActivityFrameV1, AgentScopeV1, AssistantMessageV1, BlobAttachmentV1,
    BlobMediaTypeV1, CaptureModeV1, CapturedContentV1, FailureCodeV1, FailureInfoV1,
    InlineContentV1, MODEL_CALL_RETRY_FAILURE_MESSAGE, ModelCallFinishedV1, ModelCallRetryingV1,
    ModelCallStartedV1, ModelCallStatusV1, OutputDeltaV1, PromptCaptureDispositionV1,
    PromptPreparedV1, PromptSnapshotV1, PromptToolDefinitionV1, ScopeFinishedV1, ScopeStartedV1,
    SteeringAppliedV1, SteeringSourceV1, StopReasonV1, ToolFinishedV1, ToolStartedV1, ToolStatusV1,
    TurnFinishedV1, TurnStartedV1, UsageV1,
};
use tongs::model::{ContentBlock, StopReason};
use tongs::provider::ToolDef;

use super::{ActivityClock, ProjectionSet};

mod terminal;
mod tool_result;
use terminal::scope_terminal;
use tool_result::captured_tool_result;

struct NormalizerState {
    current_turn: Option<u32>,
    pending_turn_finish: Option<(u32, u64, StopReasonV1)>,
    message_seq: u64,
}

/// The single machine-event normalizer and synchronous composite sink.
pub(super) struct NormalizingEventSink {
    scope: AgentScopeV1,
    policy: AgentActivityCapturePolicyV1,
    clock: Arc<dyn ActivityClock>,
    projections: Arc<ProjectionSet>,
    scope_started_ms: u64,
    state: Mutex<NormalizerState>,
}

impl NormalizingEventSink {
    pub(super) fn new(
        scope: AgentScopeV1,
        display_name: String,
        policy: AgentActivityCapturePolicyV1,
        clock: Arc<dyn ActivityClock>,
        projections: Arc<ProjectionSet>,
    ) -> Self {
        let scope_started_ms = clock.now().elapsed_ms;
        let sink = Self {
            scope,
            policy,
            clock,
            projections,
            scope_started_ms,
            state: Mutex::new(NormalizerState {
                current_turn: None,
                pending_turn_finish: None,
                message_seq: 0,
            }),
        };
        sink.project(
            None,
            AgentActivityEventV1::ScopeStarted(ScopeStartedV1 {
                display_name: nonempty(display_name),
            }),
        );
        sink
    }

    fn project(&self, turn: Option<u32>, event: AgentActivityEventV1) {
        self.project_with_blobs(turn, event, Vec::new());
    }

    fn project_with_blobs(
        &self,
        turn: Option<u32>,
        event: AgentActivityEventV1,
        blobs: Vec<BlobAttachmentV1>,
    ) {
        let timestamp = self.clock.now();
        let record = AgentActivityChildRecordV1 {
            frame: AgentActivityFrameV1 {
                version: ACTIVITY_PROTOCOL_VERSION,
                occurred_at: timestamp.occurred_at,
                elapsed_ms: timestamp.elapsed_ms,
                scope: self.scope.clone(),
                turn,
                event,
            },
            blobs,
        };
        let valid = if record.blobs.is_empty() {
            record.frame.validate()
        } else {
            record.validate()
        };
        if valid.is_ok() {
            self.projections.emit(&record);
        }
    }

    fn normalize(&self, event: AgentEvent) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match event {
            AgentEvent::PromptPrepared {
                system_prompt,
                initial_user_message,
                tools,
            } => self.prompt_prepared(system_prompt, initial_user_message, tools),
            AgentEvent::TurnStart { turn } => self.turn_started(&mut state, turn),
            AgentEvent::ModelCallStarted {
                turn,
                call_id,
                attempt,
                provider,
                model,
            } => self.project(
                Some(turn_number(turn)),
                AgentActivityEventV1::ModelCallStarted(ModelCallStartedV1 {
                    call_id,
                    provider,
                    model,
                    attempt,
                }),
            ),
            AgentEvent::ModelCallFinished {
                turn,
                call_id,
                attempt,
                status,
                duration_ms,
                time_to_first_token_ms,
                stop_reason,
                usage: _,
                failure: _,
            } => self.model_finished(
                &mut state,
                turn,
                call_id,
                attempt,
                status,
                duration_ms,
                time_to_first_token_ms,
                stop_reason,
            ),
            AgentEvent::ModelCallRetrying {
                turn,
                call_id,
                next_attempt,
                delay_ms,
                reason: _,
            } => self.model_retrying(&mut state, turn, call_id, next_attempt, delay_ms),
            AgentEvent::StreamDelta(delta) => self.stream_delta(&state, delta),
            AgentEvent::AssistantMessage { content } => {
                self.assistant_message(&mut state, &content)
            }
            AgentEvent::TurnUsage { turn, usage } => self.project(
                Some(turn_number(turn)),
                AgentActivityEventV1::Usage(UsageV1 {
                    input_tokens: usage.input,
                    output_tokens: usage.output,
                    cache_read_tokens: usage.cache_read,
                    cache_write_tokens: usage.cache_write,
                }),
            ),
            AgentEvent::ToolStart {
                id,
                name,
                arg_preview,
            } => self.tool_started(&state, id, name, arg_preview),
            AgentEvent::ToolEnd {
                id,
                name,
                status,
                duration_ms,
                result,
            } => self.tool_finished(&state, id, name, status, duration_ms, result),
            AgentEvent::Steered { .. } => self.project(
                state.current_turn,
                AgentActivityEventV1::SteeringApplied(SteeringAppliedV1 {
                    source: SteeringSourceV1::Worker,
                    instruction: None,
                }),
            ),
            AgentEvent::AgentEnd { reason } => self.agent_ended(&mut state, reason),
        }
    }

    fn prompt_prepared(
        &self,
        system_prompt: Option<String>,
        initial_user_message: String,
        tools: Vec<ToolDef>,
    ) {
        if self.policy.capture == CaptureModeV1::Off {
            return;
        }

        let system_prompt_present = system_prompt.is_some();
        let system_prompt_bytes = system_prompt
            .as_ref()
            .map_or(0, |prompt| prompt.len() as u64);
        let initial_user_message_bytes = initial_user_message.len() as u64;
        let tool_count = match u32::try_from(tools.len()) {
            Ok(count) => count,
            Err(_) => return,
        };
        let snapshot = PromptSnapshotV1 {
            system_prompt,
            initial_user_message,
            tools: tools
                .into_iter()
                .map(|tool| PromptToolDefinitionV1 {
                    name: tool.name,
                    description: tool.description,
                    input_schema: tool.parameters,
                })
                .collect(),
        };
        let tool_manifest_bytes = match snapshot.tools_to_canonical_json_bytes() {
            Ok(tools) => tools.len() as u64,
            Err(_) => return,
        };
        // The complete source-equivalent snapshot is serialized exactly once.
        // These bytes are moved unchanged into either inline JSON or the blob.
        let canonical = match snapshot.to_canonical_json_bytes() {
            Ok(canonical) => canonical,
            Err(_) => return,
        };
        let original_snapshot_bytes = canonical.len() as u64;

        let (disposition, captured_bytes, content, blobs) = match self.policy.capture {
            CaptureModeV1::Off => unreachable!("off returned before prompt serialization"),
            CaptureModeV1::Metadata => (
                PromptCaptureDispositionV1::OmittedPolicy,
                0,
                None,
                Vec::new(),
            ),
            CaptureModeV1::Transcript | CaptureModeV1::Diagnostic
                if canonical.len() <= self.policy.max_inline_bytes as usize =>
            {
                let Ok(text) = String::from_utf8(canonical) else {
                    return;
                };
                (
                    PromptCaptureDispositionV1::Captured,
                    original_snapshot_bytes,
                    Some(CapturedContentV1::Inline(InlineContentV1 {
                        text,
                        truncated: false,
                    })),
                    Vec::new(),
                )
            }
            CaptureModeV1::Transcript | CaptureModeV1::Diagnostic
                if original_snapshot_bytes <= self.policy.max_blob_bytes =>
            {
                let attachment =
                    BlobAttachmentV1::from_bytes(BlobMediaTypeV1::ApplicationJson, &canonical);
                let content = Some(CapturedContentV1::Blob {
                    blob: attachment.blob.clone(),
                });
                (
                    PromptCaptureDispositionV1::Captured,
                    original_snapshot_bytes,
                    content,
                    vec![attachment],
                )
            }
            CaptureModeV1::Transcript | CaptureModeV1::Diagnostic => (
                PromptCaptureDispositionV1::OmittedLimit,
                0,
                None,
                Vec::new(),
            ),
        };

        self.project_with_blobs(
            Some(0),
            AgentActivityEventV1::PromptPrepared(PromptPreparedV1 {
                system_prompt_present,
                system_prompt_bytes,
                initial_user_message_bytes,
                tool_manifest_bytes,
                tool_count,
                original_snapshot_bytes,
                captured_bytes,
                disposition,
                content,
            }),
            blobs,
        );
    }

    fn turn_started(&self, state: &mut NormalizerState, turn: usize) {
        self.flush_turn_finish(state);
        let turn = turn_number(turn);
        state.current_turn = Some(turn);
        self.project(
            Some(turn),
            AgentActivityEventV1::TurnStarted(TurnStartedV1 {}),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn model_finished(
        &self,
        state: &mut NormalizerState,
        turn: usize,
        call_id: String,
        attempt: u32,
        status: ModelCallStatus,
        duration_ms: u64,
        time_to_first_token_ms: Option<u64>,
        stop_reason: Option<StopReason>,
    ) {
        let turn = turn_number(turn);
        let stop_reason = stop_reason.map(map_stop_reason);
        self.project(
            Some(turn),
            AgentActivityEventV1::ModelCallFinished(ModelCallFinishedV1 {
                call_id,
                attempt,
                status: map_model_status(status),
                duration_ms,
                time_to_first_token_ms,
                stop_reason,
            }),
        );
        let terminal_reason = match status {
            ModelCallStatus::Succeeded => stop_reason,
            ModelCallStatus::Failed => Some(StopReasonV1::Error),
            ModelCallStatus::Cancelled => Some(StopReasonV1::Cancelled),
        };
        if let Some(stop_reason) = terminal_reason {
            state.pending_turn_finish = Some((turn, duration_ms, stop_reason));
        }
    }

    fn model_retrying(
        &self,
        state: &mut NormalizerState,
        turn: usize,
        call_id: String,
        next_attempt: u32,
        delay_ms: u64,
    ) {
        // A failed attempt looked terminal until the shell decided to retry it.
        // Keep the model-attempt boundary, but do not close the enclosing turn.
        state.pending_turn_finish = None;
        self.project(
            Some(turn_number(turn)),
            AgentActivityEventV1::ModelCallRetrying(ModelCallRetryingV1 {
                call_id,
                next_attempt,
                delay_ms,
                failure: FailureInfoV1 {
                    code: FailureCodeV1::Provider,
                    message: MODEL_CALL_RETRY_FAILURE_MESSAGE.to_string(),
                    retryable: true,
                },
            }),
        );
    }

    fn stream_delta(&self, state: &NormalizerState, delta: StreamDelta) {
        match delta {
            StreamDelta::Text(delta) if self.policy.capture == CaptureModeV1::Diagnostic => {
                self.project(
                    state.current_turn,
                    AgentActivityEventV1::OutputTextDelta(OutputDeltaV1 {
                        delta: sanitized_text(&delta, self.policy.max_inline_bytes as usize),
                    }),
                );
            }
            StreamDelta::Thinking(delta)
                if self.policy.capture == CaptureModeV1::Diagnostic
                    && self.policy.capture_thinking =>
            {
                self.project(
                    state.current_turn,
                    AgentActivityEventV1::OutputThinkingDelta(OutputDeltaV1 {
                        delta: sanitized_text(&delta, self.policy.max_inline_bytes as usize),
                    }),
                );
            }
            StreamDelta::ToolCall { .. } | StreamDelta::Text(_) | StreamDelta::Thinking(_) => {}
        }
    }

    fn assistant_message(&self, state: &mut NormalizerState, content: &[ContentBlock]) {
        if matches!(
            self.policy.capture,
            CaptureModeV1::Transcript | CaptureModeV1::Diagnostic
        ) {
            let visible = visible_assistant_text(content);
            if !visible.is_empty() && !looks_like_workspace_result(&visible) {
                state.message_seq = state.message_seq.saturating_add(1);
                self.project(
                    state.current_turn,
                    AgentActivityEventV1::AssistantMessage(AssistantMessageV1 {
                        message_id: format!("{}-message-{}", self.scope.id, state.message_seq),
                        content: CapturedContentV1::Inline(sanitized_text(
                            &visible,
                            self.policy.max_inline_bytes as usize,
                        )),
                    }),
                );
            }
        }
    }

    fn tool_started(
        &self,
        state: &NormalizerState,
        id: String,
        name: String,
        arg_preview: Option<String>,
    ) {
        // The start boundary is required in every enabled capture mode, but
        // arguments are transcript content. Metadata therefore retains only
        // the call identity and tool name; the worker independently enforces
        // the same rule for forged child frames.
        let arguments = if matches!(
            self.policy.capture,
            CaptureModeV1::Transcript | CaptureModeV1::Diagnostic
        ) {
            arg_preview.and_then(|value| {
                nonempty(value).map(|value| {
                    CapturedContentV1::Inline(sanitized_text(
                        &value,
                        self.policy.max_inline_bytes as usize,
                    ))
                })
            })
        } else {
            None
        };
        self.project(
            state.current_turn,
            AgentActivityEventV1::ToolStarted(ToolStartedV1 {
                call_id: id,
                name,
                arguments,
            }),
        );
    }

    fn tool_finished(
        &self,
        state: &NormalizerState,
        id: String,
        name: String,
        status: ToolCallStatus,
        duration_ms: u64,
        metadata: temper_agent_core::ToolResultMetadata,
    ) {
        // Never transport generic process output. Read-only bounded text tools
        // are eligible in transcript modes. `submit_for_pr` exposes only a
        // fixed acceptance marker, never the host message or gate output.
        let result = if matches!(
            self.policy.capture,
            CaptureModeV1::Transcript | CaptureModeV1::Diagnostic
        ) {
            captured_tool_result(
                &name,
                metadata.preview,
                metadata.truncated,
                self.policy.max_inline_bytes as usize,
            )
        } else {
            None
        };
        self.project(
            state.current_turn,
            AgentActivityEventV1::ToolFinished(ToolFinishedV1 {
                call_id: id,
                name,
                status: map_tool_status(status),
                duration_ms,
                result,
            }),
        );
    }

    fn agent_ended(&self, state: &mut NormalizerState, reason: AgentStop) {
        self.flush_turn_finish(state);
        let now = self.clock.now();
        let (status, terminal_reason) = scope_terminal(reason);
        self.project(
            None,
            AgentActivityEventV1::ScopeFinished(ScopeFinishedV1 {
                status,
                duration_ms: now.elapsed_ms.saturating_sub(self.scope_started_ms),
                terminal_reason: Some(terminal_reason),
            }),
        );
    }

    fn flush_turn_finish(&self, state: &mut NormalizerState) {
        if let Some((turn, duration_ms, stop_reason)) = state.pending_turn_finish.take() {
            self.project(
                Some(turn),
                AgentActivityEventV1::TurnFinished(TurnFinishedV1 {
                    duration_ms,
                    stop_reason,
                }),
            );
        }
    }
}

impl EventSink for NormalizingEventSink {
    fn emit(&self, event: AgentEvent) {
        // Activity is deliberately best effort. This guard includes policy,
        // normalization, validation, and projection code.
        let _ = catch_unwind(AssertUnwindSafe(|| self.normalize(event)));
    }
}

fn turn_number(turn: usize) -> u32 {
    u32::try_from(turn).unwrap_or(u32::MAX)
}

fn map_model_status(status: ModelCallStatus) -> ModelCallStatusV1 {
    match status {
        ModelCallStatus::Succeeded => ModelCallStatusV1::Succeeded,
        ModelCallStatus::Failed => ModelCallStatusV1::Failed,
        ModelCallStatus::Cancelled => ModelCallStatusV1::Cancelled,
    }
}

fn map_tool_status(status: ToolCallStatus) -> ToolStatusV1 {
    match status {
        ToolCallStatus::Succeeded => ToolStatusV1::Succeeded,
        ToolCallStatus::Failed => ToolStatusV1::Failed,
        ToolCallStatus::Cancelled => ToolStatusV1::Cancelled,
    }
}

fn map_stop_reason(reason: StopReason) -> StopReasonV1 {
    match reason {
        StopReason::Stop => StopReasonV1::EndTurn,
        StopReason::Length => StopReasonV1::MaxTokens,
        StopReason::ToolUse => StopReasonV1::ToolUse,
        StopReason::Error => StopReasonV1::Error,
        StopReason::Aborted => StopReasonV1::Cancelled,
    }
}

fn visible_assistant_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            // Thinking, signatures, images, and tool argument JSON are never
            // treated as visible assistant transcript content.
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn looks_like_workspace_result(value: &str) -> bool {
    if value.contains("WorkspaceResult") {
        return true;
    }
    let trimmed = value
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let Ok(serde_json::Value::Object(object)) = serde_json::from_str(trimmed) else {
        return false;
    };
    const RESULT_KEYS: &[&str] = &[
        "verdict",
        "title",
        "summary",
        "body",
        "review_body",
        "labels",
        "children",
    ];
    !object.is_empty() && object.keys().all(|key| RESULT_KEYS.contains(&key.as_str()))
}

fn sanitized_text(value: &str, maximum_bytes: usize) -> InlineContentV1 {
    let maximum_bytes = maximum_bytes.max(1);
    let redacted = crate::observability::redacted_preview(value, maximum_bytes);
    let (text, truncated_by_bytes) = truncate_owned(redacted, maximum_bytes);
    InlineContentV1 {
        text: if text.is_empty() {
            "<empty>".to_string()
        } else {
            text
        },
        truncated: truncated_by_bytes || value.len() > maximum_bytes,
    }
}

fn truncate_owned(mut value: String, maximum_bytes: usize) -> (String, bool) {
    if value.len() <= maximum_bytes {
        return (value, false);
    }
    let mut end = maximum_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    (value, true)
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}
