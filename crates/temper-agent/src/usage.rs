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
    AgentActivityEventV1, AgentActivityFrameV1, AgentScopeKindV1, CapturedContentV1,
    ModelCallStatusV1, ToolStatusV1,
};

use crate::activity::ActivityProjection;
use crate::tool_preview::tool_arg_preview;

/// The tracing target every agent observability line is emitted on.
const AGENT_TARGET: &str = "temper::agent";
const ARG_PREVIEW_BUDGET: usize = 48;

/// Builds the shell-supplied [`ArgPreviewFn`] that fills `ToolStart.arg_preview`
/// in the pure core, capturing the workspace `cwd` for repo-relative paths.
pub fn tool_arg_preview_hook(cwd: PathBuf) -> ArgPreviewFn {
    Arc::new(move |name: &str, args: &serde_json::Value| {
        tool_arg_preview(name, args, &cwd, ARG_PREVIEW_BUDGET)
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

#[derive(Clone, Copy)]
struct PendingModelFailure {
    attempt: u32,
    duration_ms: u64,
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
}

impl ActivityProjection for TracingProjection {
    fn emit(&self, frame: &AgentActivityFrameV1) {
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
                tracing::debug!(
                    target: AGENT_TARGET,
                    event = "tool.start",
                    scope = %scope,
                    scope_id = %frame.scope.id,
                    tool = %name,
                    id = %id,
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
                if tool.status == ToolStatusV1::Failed {
                    tracing::debug!(
                        target: AGENT_TARGET,
                        event = "tool.error",
                        scope = %scope,
                        scope_id = %frame.scope.id,
                        tool = %name,
                        id = %id,
                        duration_ms,
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
                self.pending_model_failures
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&(frame.scope.id.clone(), retry.call_id.clone()));
                let reason = &retry.failure.message;
                tracing::debug!(
                    target: AGENT_TARGET,
                    event = "model.call_failed",
                    scope = %scope,
                    scope_id = %frame.scope.id,
                    will_retry = true,
                    reason = %reason,
                    next_attempt = retry.next_attempt,
                    delay_ms = retry.delay_ms,
                    "agent: model call failed (will_retry=true): {reason}",
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
                    .map(|(key, failure)| (key.clone(), *failure))
                    .collect::<Vec<_>>();
                for (key, failure) in pending {
                    failures.remove(&key);
                    tracing::debug!(
                        target: AGENT_TARGET,
                        event = "model.call_failed",
                        scope = %scope,
                        scope_id = %frame.scope.id,
                        will_retry = false,
                        attempt = failure.attempt,
                        duration_ms = failure.duration_ms,
                        "agent: model call failed (will_retry=false)",
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

    fn frame(event: AgentActivityEventV1) -> AgentActivityFrameV1 {
        AgentActivityFrameV1 {
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
        }
    }

    #[test]
    fn normalized_usage_folds_into_totals() {
        let totals = Arc::new(UsageTotals::default());
        let projection = TracingProjection::new(Arc::clone(&totals));
        projection.emit(&frame(AgentActivityEventV1::ScopeStarted(ScopeStartedV1 {
            display_name: Some("main".to_string()),
        })));
        projection.emit(&frame(AgentActivityEventV1::Usage(UsageV1 {
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
        let preview = hook("read", &serde_json::json!({"path": "/ws/temper/a/b.rs"}));
        assert_eq!(preview.as_deref(), Some("a/b.rs"));
    }
}
