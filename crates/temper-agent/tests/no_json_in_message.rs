//! Regression guard for the agent-log cleanup (`docs/plans/agent-log-cleanup.md`,
//! piece E; `docs/explanation/logging-and-observability.md` §3/§5/§8).
//!
//! The original fault (issue triage of #248's run) was that `UsageLogger` dumped
//! a `StructuredEvent::render()` JSON **string** into the tracing **message**
//! body at `info`, on the legacy `temper_agent` target. §8 calls that the exact
//! anti-pattern to delete: opaque to a JSON sink, unreadable to a human,
//! unfilterable on fields. Piece D rewrote the logger onto real tracing fields at
//! `debug`/`trace`. This test installs a capturing subscriber, drives the
//! canonical normalizer/projection pipeline through a representative
//! `AgentEvent` sequence, and fails if anyone reintroduces a raw-JSON-in-message
//! line at `info`.
//!
//! Core guard: **no `info`-level event on the agent target carries a `message`
//! that starts with `{`** (raw JSON), and the between-state-cause events
//! (`turn.usage`, `tool.start`, `tool.end`, `tool.error`, `model.call_failed`,
//! `agent.end`, `usage.total`) do **not** appear at `info` at all — they belong
//! at `debug`/`trace` per §5.

use std::sync::{Arc, Mutex};

use temper_agent::activity::{AgentActivityConfig, ScopeFactory};
use temper_agent::usage::{MAIN_SCOPE, UsageTotals};
use temper_agent_core::{
    AgentEvent, AgentStop, ModelCallStatus, ModelIdentity, ToolCallStatus, ToolResultMetadata,
};
use tongs::model::Usage;
use tongs::provider::ToolDef;
use tracing::Level;
use tracing::field::{Field, Visit};
use tracing::subscriber::with_default;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry;

/// One captured tracing event: enough to assert level, target, the `message`
/// projection, and the structured `event=` key.
#[derive(Clone, Debug)]
struct Captured {
    level: Level,
    target: String,
    message: Option<String>,
    event: Option<String>,
    fields: Vec<String>,
}

impl Captured {
    /// True when this is an agent-observability line (either the canonical
    /// `temper::agent` namespace target or the legacy `temper_agent` underscore
    /// target the leak used to ride on).
    fn is_agent_target(&self) -> bool {
        self.target == "temper::agent" || self.target == "temper_agent"
    }
}

#[derive(Default)]
struct Visitor {
    message: Option<String>,
    event: Option<String>,
    fields: Vec<String>,
}

impl Visit for Visitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let rendered = format!("{value:?}");
        self.fields.push(format!("{}={rendered}", field.name()));
        match field.name() {
            "message" => self.message = Some(rendered),
            "event" => self.event = Some(trim_debug_quotes(&rendered)),
            _ => {}
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields.push(format!("{}={value}", field.name()));
        match field.name() {
            "message" => self.message = Some(value.to_string()),
            "event" => self.event = Some(value.to_string()),
            _ => {}
        }
    }
}

/// `record_debug` of a string field wraps it in quotes (`"turn.usage"`); strip
/// them so callers can compare against the bare `event=` value.
fn trim_debug_quotes(rendered: &str) -> String {
    rendered
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(rendered)
        .to_string()
}

#[derive(Clone, Default)]
struct CaptureLayer {
    events: Arc<Mutex<Vec<Captured>>>,
}

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = Visitor::default();
        event.record(&mut visitor);
        let metadata = event.metadata();
        self.events.lock().unwrap().push(Captured {
            level: *metadata.level(),
            target: metadata.target().to_string(),
            message: visitor.message,
            event: visitor.event,
            fields: visitor.fields,
        });
    }
}

/// Drives the canonical activity pipeline through a representative slice of
/// the agent event stream and returns the captured tracing events. The registry
/// has no level filter, so `info`, `debug`, and `trace` are all captured;
/// assertions then discriminate by level.
fn capture_representative_run() -> Vec<Captured> {
    let layer = CaptureLayer::default();
    let events = layer.events.clone();
    let subscriber = registry().with(layer);

    with_default(subscriber, || {
        let totals = Arc::new(UsageTotals::default());
        let scopes = ScopeFactory::new(AgentActivityConfig::default(), Arc::clone(&totals));
        let logger = scopes
            .main(
                MAIN_SCOPE,
                ModelIdentity::new("test-provider", "test-model"),
            )
            .observability
            .events;

        logger.emit(AgentEvent::PromptPrepared {
            system_prompt: Some("OPERATIONAL-PROMPT-SYSTEM-SENTINEL-364".to_string()),
            initial_user_message: "OPERATIONAL-PROMPT-USER-SENTINEL-364".to_string(),
            tools: vec![ToolDef {
                name: "sentinel_tool".to_string(),
                description: "OPERATIONAL-PROMPT-TOOL-DESCRIPTION-SENTINEL-364".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "secret_schema_field": {
                            "const": "OPERATIONAL-PROMPT-SCHEMA-SENTINEL-364"
                        }
                    }
                }),
            }],
        });
        logger.emit(AgentEvent::TurnUsage {
            turn: 0,
            usage: Usage {
                input: 470_306,
                output: 6_379,
                cache_read: 415_744,
                cache_write: 0,
                ..Usage::default()
            },
        });
        logger.emit(AgentEvent::ToolStart {
            id: "call_ok".to_string(),
            name: "read".to_string(),
            arg_preview: Some("crates/temper-config/src/resolve.rs".to_string()),
        });
        logger.emit(AgentEvent::ToolEnd {
            id: "call_ok".to_string(),
            name: "read".to_string(),
            status: ToolCallStatus::Succeeded,
            duration_ms: 12,
            result: ToolResultMetadata::default(),
        });
        logger.emit(AgentEvent::ToolStart {
            id: "call_bad".to_string(),
            name: "bash".to_string(),
            arg_preview: Some("`cargo test -p temper-config`".to_string()),
        });
        logger.emit(AgentEvent::ToolEnd {
            id: "call_bad".to_string(),
            name: "bash".to_string(),
            status: ToolCallStatus::Failed,
            duration_ms: 7,
            result: ToolResultMetadata::default(),
        });
        logger.emit(AgentEvent::ModelCallFinished {
            turn: 0,
            call_id: "turn-0".to_string(),
            attempt: 0,
            status: ModelCallStatus::Failed,
            duration_ms: 30,
            time_to_first_token_ms: None,
            stop_reason: None,
            usage: Usage::default(),
            failure: Some("stream stalled, retrying".to_string()),
        });
        logger.emit(AgentEvent::ModelCallRetrying {
            turn: 0,
            call_id: "turn-0".to_string(),
            next_attempt: 1,
            delay_ms: 500,
            reason: "stream stalled, retrying".to_string(),
        });
        logger.emit(AgentEvent::AgentEnd {
            reason: AgentStop::Completed,
        });

        // End-of-run summary on the shared (now-populated) ledger.
        totals.emit_summary();
    });

    Arc::try_unwrap(events)
        .expect("no outstanding layer references after run")
        .into_inner()
        .expect("capture mutex not poisoned")
}

#[test]
fn prompt_snapshots_have_no_operational_log_projection() {
    let captured = capture_representative_run();
    let rendered = format!("{captured:?}");
    for forbidden in [
        "OPERATIONAL-PROMPT-SYSTEM-SENTINEL-364",
        "OPERATIONAL-PROMPT-USER-SENTINEL-364",
        "OPERATIONAL-PROMPT-TOOL-DESCRIPTION-SENTINEL-364",
        "OPERATIONAL-PROMPT-SCHEMA-SENTINEL-364",
        "prompt.prepared",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "operational tracing leaked {forbidden}: {rendered}"
        );
    }
    assert!(captured.iter().all(|event| {
        event
            .fields
            .iter()
            .all(|field| !field.contains("OPERATIONAL-PROMPT"))
    }));
}

#[test]
fn no_info_level_json_in_message_on_agent_target() {
    let captured = capture_representative_run();

    // Core guard: nothing on the agent target at info may be a raw-JSON message.
    for event in captured.iter().filter(|e| e.is_agent_target()) {
        if event.level == Level::INFO {
            if let Some(message) = &event.message {
                assert!(
                    !message.trim_start().starts_with('{'),
                    "info-level JSON-in-message leaked on {}: {message:?}",
                    event.target,
                );
            }
        }
    }
}

#[test]
fn between_state_cause_events_are_not_info() {
    let captured = capture_representative_run();

    // These dotted `event=` values are §5 "causes between state changes": they
    // must render at debug/trace, never info. (The info-level run totals ride
    // the runner's `agent.finished` line — plan piece C — not this logger.)
    let must_not_be_info = [
        "turn.usage",
        "tool.start",
        "tool.end",
        "tool.error",
        "model.call_failed",
        "agent.end",
        "usage.total",
    ];

    for name in must_not_be_info {
        let at_info = captured.iter().any(|event| {
            event.is_agent_target()
                && event.level == Level::INFO
                && event.event.as_deref() == Some(name)
        });
        assert!(
            !at_info,
            "event {name:?} appeared at info on the agent target"
        );
    }

    // No agent-target event reached info at all in this run.
    let any_info = captured
        .iter()
        .any(|event| event.is_agent_target() && event.level == Level::INFO);
    assert!(
        !any_info,
        "unexpected info-level agent event(s): {:?}",
        captured
            .iter()
            .filter(|e| e.is_agent_target() && e.level == Level::INFO)
            .collect::<Vec<_>>(),
    );
}

#[test]
fn between_state_cause_events_render_human_message_at_debug() {
    let captured = capture_representative_run();

    // Stronger check: each event DOES render at debug with a human message that
    // is NOT raw JSON and carries the expected `event=` field.
    let expected_debug = [
        "turn.usage",
        "tool.start",
        "tool.end",
        "tool.error",
        "model.call_failed",
        "agent.end",
        "usage.total",
    ];

    for name in expected_debug {
        let event = captured
            .iter()
            .find(|event| {
                event.is_agent_target()
                    && event.level == Level::DEBUG
                    && event.event.as_deref() == Some(name)
            })
            .unwrap_or_else(|| panic!("expected a debug-level {name:?} event on the agent target"));

        let message = event
            .message
            .as_deref()
            .unwrap_or_else(|| panic!("event {name:?} has no human message"));
        assert!(
            !message.trim_start().starts_with('{'),
            "event {name:?} renders raw JSON in its message: {message:?}",
        );
        assert!(
            message.starts_with("agent:"),
            "event {name:?} message lacks the agent human projection: {message:?}",
        );
    }
}
