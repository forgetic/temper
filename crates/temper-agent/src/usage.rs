//! Token-usage accounting and stderr observability for coding-agent runs.
//!
//! The coding agent attaches a [`UsageLogger`] to the main run and to every
//! nested `investigate` sub-agent run. Each logger emits **real-field** tracing
//! events on the `temper::agent` target — one per model turn (`event=turn.usage`)
//! and per tool boundary (`event=tool.start` / `tool.end` / `tool.error`) — and
//! folds the per-turn token counts into a shared [`UsageTotals`], which the run
//! reports once at the end (`event=usage.total`). All of these are `debug`-level
//! "causes between state changes" per the level policy in
//! `docs/explanation/logging-and-observability.md` §5; the info-level totals are
//! carried by the runner's `agent.finished` line (plan piece C).
//!
//! These events deliberately sit **off** the closed `temper-log` `Event` catalog
//! (the info contract): their `event=` values are plain dot-namespaced strings on
//! `target: "temper::agent"`, never `temper-log` enum variants.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use temper_agent_core::{AgentEvent, ArgPreviewFn, EventSink};
use tongs::model::Usage;

use crate::tool_preview::tool_arg_preview;

/// The tracing target every agent observability line is emitted on. The `::`
/// namespace (not the legacy `temper_agent` underscore) is what `RUST_LOG` and
/// the §2 service map key off.
const AGENT_TARGET: &str = "temper::agent";

/// Character budget handed to [`tool_arg_preview`] so the rendered human line —
/// `agent:   [repo#n] tool <name> <preview>` — stays within ~80 columns once the
/// service prefix, correlation tag, and tool name are accounted for. Generous
/// enough for a typical repo-relative path; longer values truncate with `…`.
const ARG_PREVIEW_BUDGET: usize = 48;

/// Builds the shell-supplied [`ArgPreviewFn`] that fills `ToolStart.arg_preview`
/// in the pure core, capturing the workspace `cwd` for repo-relative paths.
///
/// The pure machine cannot compute the preview itself (it neither knows the
/// per-tool rendering rules nor depends on this crate's `tool_preview` module),
/// so the shell injects this closure; the core calls it with each call's name +
/// parsed arguments. See the agent-log-cleanup plan (pieces B/D).
pub fn tool_arg_preview_hook(cwd: PathBuf) -> ArgPreviewFn {
    Arc::new(move |name: &str, args: &serde_json::Value| {
        tool_arg_preview(name, args, &cwd, ARG_PREVIEW_BUDGET)
    })
}

/// A plain-`u64` snapshot of a run's [`UsageTotals`] for callers outside this
/// crate (the standalone runner enriches the `agent.finished` info line from
/// these). Decoupled from the atomic ledger so the reader gets a coherent,
/// non-aliasing copy with no lock or `Ordering` concerns.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RunTotals {
    /// Total input (prompt) tokens across the main run and all sub-agents.
    pub input: u64,
    /// Total output (completion) tokens.
    pub output: u64,
    /// Total tool calls issued.
    pub tool_calls: u64,
}

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

    /// Reads the input/output/tool-call counters into a plain [`RunTotals`].
    ///
    /// Call once at end-of-run (after the drive loop has folded every turn);
    /// the loads are `Relaxed` because the run is quiesced by then.
    pub fn snapshot(&self) -> RunTotals {
        RunTotals {
            input: self.input.load(Ordering::Relaxed),
            output: self.output.load(Ordering::Relaxed),
            tool_calls: self.tool_calls.load(Ordering::Relaxed),
        }
    }

    /// Emits the end-of-run `usage.total` line on stderr at **debug** (the
    /// info-level totals ride the runner's `agent.finished` line; plan piece C).
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

/// The scope label of the top-level agent run.
pub const MAIN_SCOPE: &str = "main";

/// An [`EventSink`] that logs turn usage and tool boundaries as real-field
/// `temper::agent` tracing events and accumulates token counts into a shared
/// [`UsageTotals`].
pub struct UsageLogger {
    scope: String,
    totals: Arc<UsageTotals>,
    /// `id` -> `(tool name, arg_preview)` recorded at `ToolStart` so a later
    /// `ToolEnd` (which only carries the id) can name the tool it ended.
    in_flight: Mutex<HashMap<String, (String, Option<String>)>>,
}

impl UsageLogger {
    pub fn new(scope: impl Into<String>, totals: Arc<UsageTotals>) -> Self {
        Self {
            scope: scope.into(),
            totals,
            in_flight: Mutex::new(HashMap::new()),
        }
    }

    /// Renders the trailing ` <preview>` segment of a tool human line, or an
    /// empty string when there is no salient argument to show.
    fn arg_suffix(arg_preview: Option<&str>) -> String {
        match arg_preview {
            Some(preview) if !preview.is_empty() => format!(" {preview}"),
            _ => String::new(),
        }
    }
}

impl EventSink for UsageLogger {
    fn emit(&self, event: AgentEvent) {
        match event {
            AgentEvent::TurnUsage { turn, usage } => {
                self.totals.add_turn(&self.scope, &usage);
                let input = usage.input;
                let output = usage.output;
                let cache_read = usage.cache_read;
                let cache_write = usage.cache_write;
                tracing::debug!(
                    target: AGENT_TARGET,
                    event = "turn.usage",
                    scope = %self.scope,
                    turn,
                    input,
                    output,
                    cache_read,
                    cache_write,
                    "agent: turn {turn} | {input} in / {output} out \
                     (cache {cache_read}r/{cache_write}w)",
                );
            }
            AgentEvent::ToolStart {
                id,
                name,
                arg_preview,
            } => {
                self.totals.tool_calls.fetch_add(1, Ordering::Relaxed);
                self.in_flight
                    .lock()
                    .expect("usage logger in-flight map")
                    .insert(id.clone(), (name.clone(), arg_preview.clone()));
                let suffix = Self::arg_suffix(arg_preview.as_deref());
                tracing::debug!(
                    target: AGENT_TARGET,
                    event = "tool.start",
                    scope = %self.scope,
                    tool = %name,
                    id = %id,
                    "agent: tool {name}{suffix}",
                );
                // The richest argument detail the shell has for this call is the
                // already-bounded, already-redacted preview (the raw args never
                // reach the sink). Carry it at trace for the wire-level view.
                if let Some(preview) = arg_preview {
                    tracing::trace!(
                        target: AGENT_TARGET,
                        event = "tool.start.args",
                        scope = %self.scope,
                        tool = %name,
                        id = %id,
                        args = %preview,
                        "agent: tool {name} args {preview}",
                    );
                }
            }
            AgentEvent::ToolEnd { id, is_error } => {
                let recalled = self
                    .in_flight
                    .lock()
                    .expect("usage logger in-flight map")
                    .remove(&id);
                let (tool, arg_preview) = recalled.unwrap_or_default();
                let suffix = Self::arg_suffix(arg_preview.as_deref());
                if is_error {
                    tracing::debug!(
                        target: AGENT_TARGET,
                        event = "tool.error",
                        scope = %self.scope,
                        tool = %tool,
                        id = %id,
                        "agent: tool {tool}{suffix} error",
                    );
                } else {
                    tracing::debug!(
                        target: AGENT_TARGET,
                        event = "tool.end",
                        scope = %self.scope,
                        tool = %tool,
                        id = %id,
                        "agent: tool {tool}{suffix} done",
                    );
                }
            }
            AgentEvent::ModelCallFailed { reason, will_retry } => {
                let reason = crate::observability::redacted_preview(&reason, 300);
                tracing::debug!(
                    target: AGENT_TARGET,
                    event = "model.call_failed",
                    scope = %self.scope,
                    will_retry,
                    reason = %reason,
                    "agent: model call failed (will_retry={will_retry}): {reason}",
                );
            }
            AgentEvent::AgentEnd { reason } => {
                tracing::debug!(
                    target: AGENT_TARGET,
                    event = "agent.end",
                    scope = %self.scope,
                    reason = ?reason,
                    "agent: run ended ({reason:?})",
                );
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logger() -> UsageLogger {
        UsageLogger::new(MAIN_SCOPE, Arc::new(UsageTotals::default()))
    }

    #[test]
    fn arg_suffix_renders_space_prefixed_preview_or_empty() {
        assert_eq!(UsageLogger::arg_suffix(Some("src/main.rs")), " src/main.rs");
        assert_eq!(UsageLogger::arg_suffix(Some("")), "");
        assert_eq!(UsageLogger::arg_suffix(None), "");
    }

    #[test]
    fn tool_start_records_name_and_preview_for_later_tool_end() {
        let logger = logger();
        logger.emit(AgentEvent::ToolStart {
            id: "call_1".to_string(),
            name: "read".to_string(),
            arg_preview: Some("crates/x/src/y.rs".to_string()),
        });
        let recalled = logger.in_flight.lock().expect("map").get("call_1").cloned();
        assert_eq!(
            recalled,
            Some(("read".to_string(), Some("crates/x/src/y.rs".to_string())))
        );
        // tool_calls counter advanced exactly once.
        assert_eq!(logger.totals.tool_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn tool_end_consumes_the_in_flight_entry() {
        let logger = logger();
        logger.emit(AgentEvent::ToolStart {
            id: "call_2".to_string(),
            name: "bash".to_string(),
            arg_preview: None,
        });
        logger.emit(AgentEvent::ToolEnd {
            id: "call_2".to_string(),
            is_error: true,
        });
        assert!(
            logger
                .in_flight
                .lock()
                .expect("map")
                .get("call_2")
                .is_none(),
            "ToolEnd should remove the in-flight entry",
        );
    }

    #[test]
    fn turn_usage_folds_into_totals() {
        let logger = logger();
        logger.emit(AgentEvent::TurnUsage {
            turn: 0,
            usage: Usage {
                input: 100,
                output: 20,
                cache_read: 5,
                cache_write: 1,
                ..Usage::default()
            },
        });
        assert_eq!(logger.totals.input.load(Ordering::Relaxed), 100);
        assert_eq!(logger.totals.output.load(Ordering::Relaxed), 20);
        assert_eq!(logger.totals.turns.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn arg_preview_hook_renders_repo_relative_path() {
        let hook = tool_arg_preview_hook(PathBuf::from("/ws/temper"));
        let preview = hook("read", &serde_json::json!({"path": "/ws/temper/a/b.rs"}));
        assert_eq!(preview.as_deref(), Some("a/b.rs"));
    }
}
