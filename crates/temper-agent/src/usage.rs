//! Usage accounting and operational tracing projected from canonical activity.
//!
//! The machine event stream is normalized exactly once in [`crate::activity`].
//! This module does not implement `temper_agent_core::EventSink`; it consumes
//! the shared typed frames alongside the optional activity transport.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use temper_agent_core::ArgPreviewFn;
use temper_protocol_activity::{
    AgentActivityChildRecordV1, AgentActivityEventV1, AgentActivityFrameV1, AgentScopeKindV1,
    CapturedContentV1, InlineContentV1, ModelCallStatusV1, ModelFailureV1, ToolStatusV1,
};

use crate::activity::ActivityProjection;
use crate::tool_preview::tool_start_presentation;

/// The tracing target every agent observability line is emitted on.
const AGENT_TARGET: &str = "temper::agent";
const ARG_PREVIEW_BUDGET: usize = 48;

/// Builds the shell-supplied [`ArgPreviewFn`] that finalizes the separate
/// human preview and diagnostic shell evidence for each `ToolStart`.
pub fn tool_arg_preview_hook(cwd: PathBuf) -> ArgPreviewFn {
    Arc::new(move |name: &str, args: &serde_json::Value| {
        tool_start_presentation(name, args, &cwd, ARG_PREVIEW_BUDGET)
    })
}

/// A plain-`u64` snapshot of a run's [`UsageTotals`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RunTotals {
    pub input: u64,
    pub output: u64,
    pub tool_calls: u64,
}

/// Aggregated token/tool counters for one coding-agent run (main run plus all
/// nested sub-agent scopes).
#[derive(Default)]
pub struct UsageTotals {
    input: AtomicU64,
    output: AtomicU64,
    cache_read: AtomicU64,
    cache_write: AtomicU64,
    turns: AtomicU64,
    tool_calls: AtomicU64,
    sub_agent_turns: AtomicU64,
}

impl UsageTotals {
    fn add_turn(
        &self,
        scope_kind: AgentScopeKindV1,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
    ) {
        self.input.fetch_add(input, Ordering::Relaxed);
        self.output.fetch_add(output, Ordering::Relaxed);
        self.cache_read.fetch_add(cache_read, Ordering::Relaxed);
        self.cache_write.fetch_add(cache_write, Ordering::Relaxed);
        self.turns.fetch_add(1, Ordering::Relaxed);
        if scope_kind == AgentScopeKindV1::SubAgent {
            self.sub_agent_turns.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn snapshot(&self) -> RunTotals {
        RunTotals {
            input: self.input.load(Ordering::Relaxed),
            output: self.output.load(Ordering::Relaxed),
            tool_calls: self.tool_calls.load(Ordering::Relaxed),
        }
    }

    pub fn emit_summary(&self) {
        let input = self.input.load(Ordering::Relaxed);
        let output = self.output.load(Ordering::Relaxed);
        let cache_read = self.cache_read.load(Ordering::Relaxed);
        let cache_write = self.cache_write.load(Ordering::Relaxed);
        let turns = self.turns.load(Ordering::Relaxed);
        let tool_calls = self.tool_calls.load(Ordering::Relaxed);
        let sub_agent_turns = self.sub_agent_turns.load(Ordering::Relaxed);
        tracing::debug!(
            target: AGENT_TARGET,
            event = "usage.total",
            input,
            output,
            cache_read,
            cache_write,
            turns,
            tool_calls,
            sub_agent_turns,
            "agent: usage_total | {input} in / {output} out, cache {cache_read}r/{cache_write}w \
             | {turns} turns, {tool_calls} tool calls",
        );
    }
}

/// Display label retained separately from the unique main scope ID.
pub const MAIN_SCOPE: &str = "main";

/// Operational tracing and totals projection over canonical activity frames.
pub(crate) struct TracingProjection {
    totals: Arc<UsageTotals>,
    display_names: Mutex<HashMap<String, String>>,
    in_flight: Mutex<HashMap<(String, String), Option<String>>>,
    pending_model_failures: Mutex<HashMap<(String, String), PendingModelFailure>>,
}

#[derive(Clone)]
struct PendingModelFailure {
    attempt: u32,
    duration_ms: u64,
    diagnostic: ModelFailureV1,
}

impl TracingProjection {
    pub(crate) fn new(totals: Arc<UsageTotals>) -> Self {
        Self {
            totals,
            display_names: Mutex::new(HashMap::new()),
            in_flight: Mutex::new(HashMap::new()),
            pending_model_failures: Mutex::new(HashMap::new()),
        }
    }

    fn scope_label(&self, frame: &AgentActivityFrameV1) -> String {
        self.display_names
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&frame.scope.id)
            .cloned()
            .unwrap_or_else(|| match frame.scope.kind {
                AgentScopeKindV1::Main => MAIN_SCOPE.to_string(),
                AgentScopeKindV1::SubAgent => "sub-agent".to_string(),
            })
    }

    fn content_text(content: &CapturedContentV1) -> Option<&str> {
        match content {
            CapturedContentV1::Inline(inline) => Some(inline.text.as_str()),
            CapturedContentV1::Blob { .. } => None,
        }
    }

    fn arg_suffix(arg_preview: Option<&str>) -> String {
        match arg_preview {
            Some(preview) if !preview.is_empty() => format!(" {preview}"),
            _ => String::new(),
        }
    }

    fn failure_detail(failure: &ModelFailureV1) -> String {
        let mut detail = format!(
            "{}/{} category={} disposition={} boundary={} event_kind={} retryable={} status_present={} code_present={}",
            failure.provider,
            failure.model,
            failure.category.as_str(),
            failure.disposition.as_str(),
            failure.boundary.as_str(),
            failure.event_kind.as_str(),
            failure.retryable,
            failure.status_present,
            failure.code_present,
        );
        if let Some(status) = failure.http_status {
            detail.push_str(&format!(" http_status={status}"));
        }
        if let Some(request_id) = &failure.provider_request_id {
            detail.push_str(&format!(" request_id={request_id}"));
        }
        if let Some(code) = &failure.provider_error_code {
            detail.push_str(&format!(" provider_code={code}"));
        }
        if failure.detail_redacted {
            detail.push_str(" detail_redacted=true");
        }
        detail.push_str(": ");
        detail.push_str(&failure.message);
        detail
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_model_failure(
        scope: &str,
        scope_id: &str,
        call_id: &str,
        will_retry: bool,
        attempt: u32,
        duration_ms: u64,
        next_attempt: Option<u32>,
        delay_ms: u64,
        failure: &ModelFailureV1,
    ) {
        if will_retry {
            let http_status = failure.http_status.map(u64::from);
            tracing::debug!(
                target: AGENT_TARGET,
                service = "agent",
                event = "model.turn.retrying",
                scope,
                scope_id,
                call_id,
                attempt,
                next_attempt = next_attempt.unwrap_or_else(|| attempt.saturating_add(1)),
                delay_ms,
                duration_ms,
                disposition = failure.disposition.as_str(),
                final_disposition = failure.disposition.as_str(),
                boundary = failure.boundary.as_str(),
                event_kind = failure.event_kind.as_str(),
                status_present = failure.status_present,
                code_present = failure.code_present,
                provider = failure.provider.as_str(),
                model = failure.model.as_str(),
                category = failure.category.as_str(),
                retryable = failure.retryable,
                http_status,
                provider_request_id = failure.provider_request_id.as_deref(),
                provider_error_code = failure.provider_error_code.as_deref(),
                detail_redacted = failure.detail_redacted,
                "agent: retrying failed model turn after bounded backoff"
            );
            return;
        }
        let detail = Self::failure_detail(failure);
        let http_status = failure.http_status.map(u64::from);
        tracing::debug!(
            target: AGENT_TARGET,
            event = "model.call_failed",
            scope,
            scope_id,
            will_retry,
            attempt,
            duration_ms,
            next_attempt,
            delay_ms,
            model.provider = %failure.provider,
            model.name = %failure.model,
            model.failure.category = failure.category.as_str(),
            model.failure.disposition = failure.disposition.as_str(),
            model.failure.boundary = failure.boundary.as_str(),
            model.failure.event_kind = failure.event_kind.as_str(),
            model.failure.status_present = failure.status_present,
            model.failure.code_present = failure.code_present,
            model.failure.retryable = failure.retryable,
            model.failure.http_status = http_status,
            model.failure.request_id = failure.provider_request_id.as_deref().unwrap_or(""),
            model.failure.provider_code = failure.provider_error_code.as_deref().unwrap_or(""),
            model.failure.detail_redacted = failure.detail_redacted,
            model.failure.message = %failure.message,
            reason = %failure.message,
            "agent: model call failed (will_retry={will_retry}): {detail}",
        );
    }
}

impl ActivityProjection for TracingProjection {
    fn emit_tool_started(
        &self,
        record: &AgentActivityChildRecordV1,
        human_arg_preview: Option<&str>,
    ) {
        // Diagnostic activity may carry a complete shell command. Operational
        // tracing must always receive only the independently finalized short
        // preview, regardless of durable capture mode.
        let mut human_record = record.clone();
        if let AgentActivityEventV1::ToolStarted(started) = &mut human_record.frame.event {
            started.arguments = human_arg_preview
                .filter(|value| !value.is_empty())
                .map(|value| {
                    CapturedContentV1::Inline(InlineContentV1 {
                        text: value.to_string(),
                        truncated: false,
                    })
                });
        }
        self.emit(&human_record);
    }

    fn emit(&self, record: &AgentActivityChildRecordV1) {
        // Prompt attachments are source-equivalent data and are intentionally
        // outside operational tracing and usage accounting.
        let frame = &record.frame;
        if let AgentActivityEventV1::ScopeStarted(started) = &frame.event {
            if let Some(name) = &started.display_name {
                self.display_names
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(frame.scope.id.clone(), name.clone());
            }
        }
        let scope = self.scope_label(frame);
        match &frame.event {
            // Prompt snapshots are source-equivalent content. Operational
            // tracing deliberately emits no event and reads no body fields;
            // durable authorized trace query/export is their only projection.
            AgentActivityEventV1::PromptPrepared(_) => {}
            AgentActivityEventV1::Usage(usage) => {
                self.totals.add_turn(
                    frame.scope.kind,
                    usage.input_tokens,
                    usage.output_tokens,
                    usage.cache_read_tokens,
                    usage.cache_write_tokens,
                );
                let turn = frame.turn.unwrap_or_default();
                let input = usage.input_tokens;
                let output = usage.output_tokens;
                let cache_read = usage.cache_read_tokens;
                let cache_write = usage.cache_write_tokens;
                tracing::debug!(
                    target: AGENT_TARGET,
                    event = "turn.usage",
                    scope = %scope,
                    scope_id = %frame.scope.id,
                    turn,
                    input,
                    output,
                    cache_read,
                    cache_write,
                    "agent: turn {turn} | {input} in / {output} out \
                     (cache {cache_read}r/{cache_write}w)",
                );
            }
            AgentActivityEventV1::ToolStarted(tool) => {
                self.totals.tool_calls.fetch_add(1, Ordering::Relaxed);
                let preview = tool
                    .arguments
                    .as_ref()
                    .and_then(Self::content_text)
                    .map(str::to_string);
                self.in_flight
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(
                        (frame.scope.id.clone(), tool.call_id.clone()),
                        preview.clone(),
                    );
                let suffix = Self::arg_suffix(preview.as_deref());
                let name = &tool.name;
                let id = &tool.call_id;
                let arguments_present = preview.is_some();
                let shell_discovery_disposition = tool.shell_discovery_disposition;
                let shell_discovery_status = shell_discovery_disposition.map(|value| {
                    match value.status {
                        temper_protocol_activity::ShellDiscoveryDispositionStatusV1::ExcludedNeverExecutedLocalPolicyDenial => {
                            "excluded_never_executed_local_policy_denial"
                        }
                    }
                });
                tracing::debug!(
                    target: AGENT_TARGET,
                    event = "tool.start",
                    scope = %scope,
                    scope_id = %frame.scope.id,
                    tool = %name,
                    id = %id,
                    tool.arguments.present = arguments_present,
                    tool.shell_discovery_disposition.version = shell_discovery_disposition.map(|value| value.version),
                    tool.shell_discovery_disposition.status = shell_discovery_status,
                    tool.shell_discovery_disposition.matching_discovery_segments = shell_discovery_disposition.map(|value| value.matching_discovery_segments),
                    "agent: tool {name}{suffix}",
                );
                if let Some(preview) = preview {
                    tracing::trace!(
                        target: AGENT_TARGET,
                        event = "tool.start.args",
                        scope = %scope,
                        scope_id = %frame.scope.id,
                        tool = %name,
                        id = %id,
                        args = %preview,
                        "agent: tool {name} args {preview}",
                    );
                }
            }
            AgentActivityEventV1::ToolFinished(tool) => {
                let preview = self
                    .in_flight
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&(frame.scope.id.clone(), tool.call_id.clone()))
                    .flatten();
                let suffix = Self::arg_suffix(preview.as_deref());
                let name = &tool.name;
                let id = &tool.call_id;
                let duration_ms = tool.duration_ms;
                if tool.status != ToolStatusV1::Succeeded {
                    let failure_category = tool.failure.as_ref().map(|value| value.category);
                    let failure_reason = tool.failure.as_ref().map(|value| value.reason);
                    let retry_disposition =
                        tool.failure.as_ref().map(|value| value.retry_disposition);
                    let retryable = tool.failure.as_ref().map(|value| value.retryable);
                    let conventional_fallback = tool
                        .failure
                        .as_ref()
                        .map(|value| value.fallback_to_conventional_discovery);
                    let failure_message = tool.failure.as_ref().map(|value| value.message.as_str());
                    let graph_reason = tool
                        .failure
                        .as_ref()
                        .and_then(|value| value.graph_exploration.as_ref())
                        .map(|value| match value.reason {
                            temper_protocol_activity::GraphExplorationClosedReasonV1::Completed => {
                                "completed"
                            }
                            temper_protocol_activity::GraphExplorationClosedReasonV1::RecoverableIncompleteEvidence => {
                                "recoverable_incomplete_evidence"
                            }
                            temper_protocol_activity::GraphExplorationClosedReasonV1::RecoveryExhausted => {
                                "recovery_exhausted"
                            }
                        });
                    let graph_missing = tool.failure.as_ref().and_then(|value| {
                        value.graph_exploration.as_ref().map(|details| {
                            details
                                .missing_evidence
                                .iter()
                                .map(|kind| kind.as_str())
                                .collect::<Vec<_>>()
                                .join(",")
                        })
                    });
                    let graph_action = tool.failure.as_ref().and_then(|value| {
                        value
                            .graph_exploration
                            .as_ref()
                            .map(|details| details.permitted_action.as_str())
                    });
                    let graph_remaining = tool.failure.as_ref().and_then(|value| {
                        value
                            .graph_exploration
                            .as_ref()
                            .map(|details| details.remaining_allowance)
                    });
                    tracing::debug!(
                        target: AGENT_TARGET,
                        event = "tool.error",
                        scope = %scope,
                        scope_id = %frame.scope.id,
                        tool = %name,
                        id = %id,
                        duration_ms,
                        tool.failure.category = failure_category.map(|value| value.as_str()),
                        tool.failure.reason = failure_reason.map(|value| value.as_str()),
                        tool.failure.retry_disposition = retry_disposition.map(|value| value.as_str()),
                        tool.failure.retryable = retryable,
                        tool.failure.conventional_fallback = conventional_fallback,
                        tool.failure.message = failure_message,
                        tool.failure.graph.reason = graph_reason,
                        tool.failure.graph.missing_evidence = graph_missing.as_deref(),
                        tool.failure.graph.permitted_action = graph_action,
                        tool.failure.graph.remaining_allowance = graph_remaining,
                        "agent: tool {name}{suffix} error",
                    );
                } else {
                    tracing::debug!(
                        target: AGENT_TARGET,
                        event = "tool.end",
                        scope = %scope,
                        scope_id = %frame.scope.id,
                        tool = %name,
                        id = %id,
                        duration_ms,
                        "agent: tool {name}{suffix} done",
                    );
                }
            }
            AgentActivityEventV1::ModelCallRetrying(retry) => {
                let pending = self
                    .pending_model_failures
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&(frame.scope.id.clone(), retry.call_id.clone()))
                    .unwrap_or_else(|| PendingModelFailure {
                        attempt: retry.next_attempt.saturating_sub(1),
                        duration_ms: 0,
                        diagnostic: ModelFailureV1::redacted_unknown("unknown", "unknown", false),
                    });
                Self::emit_model_failure(
                    &scope,
                    &frame.scope.id,
                    &retry.call_id,
                    true,
                    pending.attempt,
                    pending.duration_ms,
                    Some(retry.next_attempt),
                    retry.delay_ms,
                    &pending.diagnostic,
                );
            }
            AgentActivityEventV1::ModelCallFinished(finished)
                if finished.status == ModelCallStatusV1::Failed =>
            {
                self.pending_model_failures
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(
                        (frame.scope.id.clone(), finished.call_id.clone()),
                        PendingModelFailure {
                            attempt: finished.attempt,
                            duration_ms: finished.duration_ms,
                            diagnostic: finished.failure.clone().unwrap_or_else(|| {
                                ModelFailureV1::redacted_unknown("unknown", "unknown", false)
                            }),
                        },
                    );
            }
            AgentActivityEventV1::ModelCallFinished(finished) => {
                self.pending_model_failures
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&(frame.scope.id.clone(), finished.call_id.clone()));
            }
            AgentActivityEventV1::ScopeFinished(finished) => {
                let mut failures = self
                    .pending_model_failures
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let pending = failures
                    .iter()
                    .filter(|((scope_id, _), _)| scope_id == &frame.scope.id)
                    .map(|(key, failure)| (key.clone(), failure.clone()))
                    .collect::<Vec<_>>();
                for (key, failure) in pending {
                    failures.remove(&key);
                    Self::emit_model_failure(
                        &scope,
                        &frame.scope.id,
                        &key.1,
                        false,
                        failure.attempt,
                        failure.duration_ms,
                        None,
                        0,
                        &failure.diagnostic,
                    );
                }
                drop(failures);
                let reason = format!("{:?}", finished.status);
                tracing::debug!(
                    target: AGENT_TARGET,
                    event = "agent.end",
                    scope = %scope,
                    scope_id = %frame.scope.id,
                    reason = %reason,
                    duration_ms = finished.duration_ms,
                    "agent: run ended ({reason})",
                );
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_protocol_activity::{
        ACTIVITY_PROTOCOL_VERSION, AgentActivityEventV1, AgentScopeV1, ScopeStartedV1, UsageV1,
    };

    fn record(event: AgentActivityEventV1) -> AgentActivityChildRecordV1 {
        AgentActivityChildRecordV1 {
            frame: AgentActivityFrameV1 {
                version: ACTIVITY_PROTOCOL_VERSION,
                occurred_at: "2026-01-02T03:04:05.000Z".to_string(),
                elapsed_ms: 1,
                scope: AgentScopeV1 {
                    id: "scope-1".to_string(),
                    kind: AgentScopeKindV1::Main,
                    parent_id: None,
                },
                turn: Some(0),
                event,
            },
            blobs: Vec::new(),
        }
    }

    #[test]
    fn normalized_usage_folds_into_totals() {
        let totals = Arc::new(UsageTotals::default());
        let projection = TracingProjection::new(Arc::clone(&totals));
        projection.emit(&record(AgentActivityEventV1::ScopeStarted(
            ScopeStartedV1 {
                display_name: Some("main".to_string()),
            },
        )));
        projection.emit(&record(AgentActivityEventV1::Usage(UsageV1 {
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 5,
            cache_write_tokens: 1,
        })));
        assert_eq!(totals.snapshot().input, 100);
        assert_eq!(totals.snapshot().output, 20);
        assert_eq!(totals.turns.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn arg_preview_hook_renders_repo_relative_path() {
        let hook = tool_arg_preview_hook(PathBuf::from("/ws/temper"));
        let presentation = hook("read", &serde_json::json!({"path": "/ws/temper/a/b.rs"}));
        assert_eq!(presentation.arg_preview.as_deref(), Some("a/b.rs"));
        assert_eq!(presentation.diagnostic_arguments, None);
    }
}

#[cfg(test)]
#[path = "usage_projection_tests.rs"]
mod projection_tests;
