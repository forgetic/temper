//! Token-usage accounting and stderr observability for coding-agent runs.
//!
//! The coding agent attaches a [`UsageLogger`] to the main run and to every
//! nested `investigate` sub-agent run. Each logger emits one structured JSON
//! line per model turn (`event=turn_usage`) and per tool start
//! (`event=tool_start`) on **stderr** — stdout stays reserved for the
//! step-progress protocol stream — and folds the per-turn token counts into a
//! shared [`UsageTotals`], which the run reports once at the end
//! (`event=usage_total`). The totals make anvil runs directly comparable with
//! other agents (wall-clock + tokens), and the per-turn lines show how the
//! loop spends its budget.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use temper_agent_core::{AgentEvent, EventSink};
use tongs::model::Usage;

use crate::observability::StructuredEvent;

/// Aggregated token/tool counters for one coding-agent run (main run plus all
/// nested sub-agent runs).
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
    fn add_turn(&self, scope: &str, usage: &Usage) {
        self.input.fetch_add(usage.input, Ordering::Relaxed);
        self.output.fetch_add(usage.output, Ordering::Relaxed);
        self.cache_read
            .fetch_add(usage.cache_read, Ordering::Relaxed);
        self.cache_write
            .fetch_add(usage.cache_write, Ordering::Relaxed);
        self.turns.fetch_add(1, Ordering::Relaxed);
        if scope != MAIN_SCOPE {
            self.sub_agent_turns.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Emits the end-of-run `usage_total` line on stderr.
    pub fn emit_summary(&self) {
        tracing::info!(
            target: "temper_agent",
            "{}",
            StructuredEvent::new("usage_total")
                .u64("input", self.input.load(Ordering::Relaxed))
                .u64("output", self.output.load(Ordering::Relaxed))
                .u64("cache_read", self.cache_read.load(Ordering::Relaxed))
                .u64("cache_write", self.cache_write.load(Ordering::Relaxed))
                .u64("turns", self.turns.load(Ordering::Relaxed))
                .u64("tool_calls", self.tool_calls.load(Ordering::Relaxed))
                .u64(
                    "sub_agent_turns",
                    self.sub_agent_turns.load(Ordering::Relaxed)
                )
                .render()
        );
    }
}

/// The scope label of the top-level agent run.
pub const MAIN_SCOPE: &str = "main";

/// An [`EventSink`] that logs turn usage and tool starts as structured stderr
/// lines and accumulates token counts into a shared [`UsageTotals`].
pub struct UsageLogger {
    scope: String,
    totals: Arc<UsageTotals>,
}

impl UsageLogger {
    pub fn new(scope: impl Into<String>, totals: Arc<UsageTotals>) -> Self {
        Self {
            scope: scope.into(),
            totals,
        }
    }
}

impl EventSink for UsageLogger {
    fn emit(&self, event: AgentEvent) {
        match event {
            AgentEvent::TurnUsage { turn, usage } => {
                self.totals.add_turn(&self.scope, &usage);
                tracing::debug!(
                    target: "temper_agent",
                    "{}",
                    StructuredEvent::new("turn_usage")
                        .str("scope", &self.scope)
                        .usize("turn", turn)
                        .u64("input", usage.input)
                        .u64("output", usage.output)
                        .u64("cache_read", usage.cache_read)
                        .u64("cache_write", usage.cache_write)
                        .render()
                );
            }
            AgentEvent::ToolStart { id, name } => {
                self.totals.tool_calls.fetch_add(1, Ordering::Relaxed);
                tracing::debug!(
                    target: "temper_agent",
                    "{}",
                    StructuredEvent::new("tool_start")
                        .str("scope", &self.scope)
                        .str("id", &id)
                        .str("tool", &name)
                        .render()
                );
            }
            AgentEvent::ToolEnd { id, is_error: true } => {
                tracing::info!(
                    target: "temper_agent",
                    "{}",
                    StructuredEvent::new("tool_error")
                        .str("scope", &self.scope)
                        .str("id", &id)
                        .render()
                );
            }
            AgentEvent::ToolEnd {
                id,
                is_error: false,
            } => {
                tracing::debug!(
                    target: "temper_agent",
                    "{}",
                    StructuredEvent::new("tool_end")
                        .str("scope", &self.scope)
                        .str("id", &id)
                        .render()
                );
            }
            AgentEvent::ModelCallFailed { reason, will_retry } => {
                tracing::info!(
                    target: "temper_agent",
                    "{}",
                    StructuredEvent::new("model_call_failed")
                        .str("scope", &self.scope)
                        .bool("will_retry", will_retry)
                        .str(
                            "reason",
                            crate::observability::redacted_preview(&reason, 300),
                        )
                        .render()
                );
            }
            AgentEvent::AgentEnd { reason } => {
                tracing::info!(
                    target: "temper_agent",
                    "{}",
                    StructuredEvent::new("agent_end")
                        .str("scope", &self.scope)
                        .str("reason", format!("{reason:?}"))
                        .render()
                );
            }
            _ => {}
        }
    }
}
