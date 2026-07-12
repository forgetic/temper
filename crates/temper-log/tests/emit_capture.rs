// SPDX-License-Identifier: MPL-2.0

//! End-to-end check that an `emit_*` constructor produces the expected
//! structured fields *and* the §7 human message at one site.
//!
//! A custom capturing [`Layer`] records every field of each event (including the
//! generated `message`) into a shared buffer; the test then asserts both the
//! machine fields (`event`, `service`, `artifact.ref`, …) and the human line are
//! present and agree. This is the §3-rule-3 invariant under test: the two
//! projections come from one emit site and cannot disagree.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use temper_log::WorkItemRef;
use temper_log::emit::{self, ForgeContextRead, TransitionApplied};
use tracing::field::{Field, Visit};
use tracing::subscriber::with_default;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry;

/// One captured event: its target plus all field name → rendered value pairs.
#[derive(Clone, Debug, Default)]
struct Captured {
    target: String,
    fields: BTreeMap<String, String>,
}

#[derive(Default)]
struct Visitor {
    fields: BTreeMap<String, String>,
}

impl Visit for Visitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

#[derive(Clone, Default)]
struct CaptureLayer {
    events: Arc<Mutex<Vec<Captured>>>,
}

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = Visitor::default();
        event.record(&mut visitor);
        self.events.lock().unwrap().push(Captured {
            target: event.metadata().target().to_string(),
            fields: visitor.fields,
        });
    }
}

#[test]
fn transition_applied_emits_fields_and_human_message() {
    let layer = CaptureLayer::default();
    let events = layer.events.clone();
    let subscriber = registry().with(layer);

    with_default(subscriber, || {
        let item = WorkItemRef::issue("acme/widgets", 42);
        emit::emit_transition_applied(TransitionApplied {
            item: &item,
            transition: "triage_intake_to_code",
            detail: "body rewritten as code spec",
            labels_delta: "-untriaged +code +ready",
        });
    });

    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1, "exactly one event emitted");
    let ev = &captured[0];

    // Correct tracing target for the engine service.
    assert_eq!(ev.target, "temper::engine");

    // The §3 machine fields are real tracing fields (not buried in a string).
    assert_eq!(ev.fields.get("service").map(String::as_str), Some("engine"));
    assert_eq!(
        ev.fields.get("event").map(String::as_str),
        Some("transition.applied")
    );
    assert_eq!(
        ev.fields.get("repo").map(String::as_str),
        Some("acme/widgets")
    );
    // Dotted field names survive as dotted keys.
    assert_eq!(
        ev.fields.get("artifact.ref").map(String::as_str),
        Some("acme/widgets#42")
    );
    assert_eq!(
        ev.fields.get("transition").map(String::as_str),
        Some("triage_intake_to_code")
    );
    assert_eq!(
        ev.fields.get("labels.delta").map(String::as_str),
        Some("-untriaged +code +ready")
    );

    // The human message is the §7 line: the padded `engine:  ` service prefix
    // followed by the exact §7 body, generated from the same inputs as the
    // fields. The prefix is an emit-site concern (the `human::*` body stays
    // prefix-less); the structured fields above carry NO prefix.
    assert_eq!(
        ev.fields.get("message").map(String::as_str),
        Some(
            "engine:  [acme/widgets#42] triage_intake_to_code applied | body rewritten as code spec | -untriaged +code +ready"
        )
    );
}

#[test]
fn forge_context_read_audit_is_structured_and_never_accepts_sensitive_content() {
    let layer = CaptureLayer::default();
    let events = layer.events.clone();
    let subscriber = registry().with(layer);

    with_default(subscriber, || {
        emit::emit_forge_context_read(ForgeContextRead {
            worker_id: "worker-a",
            job_id: "job-283",
            role: "engineer",
            operation: "forge_get_item",
            repository: "ai/temper",
            item_number: 283,
            status: "success",
            result_count: 1,
            truncated: false,
            duration_ms: 12,
        });
    });

    let captured = events.lock().unwrap();
    assert_eq!(captured.len(), 1);
    let event = &captured[0];
    assert_eq!(event.target, "temper::engine");
    for (field, expected) in [
        ("service", "engine"),
        ("event", "forge.context.read"),
        ("worker_id", "worker-a"),
        ("job_id", "job-283"),
        ("role", "engineer"),
        ("operation", "forge_get_item"),
        ("repo", "ai/temper"),
        ("item_number", "283"),
        ("status", "success"),
        ("result_count", "1"),
        ("duration_ms", "12"),
    ] {
        assert_eq!(event.fields.get(field).map(String::as_str), Some(expected));
    }
    assert_eq!(
        event.fields.get("truncated").map(String::as_str),
        Some("false")
    );
    let rendered = format!("{:?}", event.fields);
    for forbidden in [
        "sensitive body",
        "private comment",
        "Authorization",
        "Bearer",
        "pool-secret",
    ] {
        assert!(!rendered.contains(forbidden), "audit leaked {forbidden}");
    }
}

#[test]
fn lease_claimed_emits_worker_target_and_join_key() {
    let layer = CaptureLayer::default();
    let events = layer.events.clone();
    let subscriber = registry().with(layer);

    with_default(subscriber, || {
        let item = WorkItemRef::issue("acme/api", 7);
        emit::emit_lease_claimed(emit::LeaseClaimed {
            item: &item,
            role: "mechanical",
            ttl_human: "10m",
            ttl_ms: 600_000,
            running: "mark_untriaged",
        });
    });

    let captured = events.lock().unwrap();
    let ev = &captured[0];
    assert_eq!(ev.target, "temper::worker");
    assert_eq!(ev.fields.get("service").map(String::as_str), Some("worker"));
    assert_eq!(
        ev.fields.get("event").map(String::as_str),
        Some("lease.claimed")
    );
    // The numeric TTL stays numeric in the field, formatted only in the message.
    assert_eq!(ev.fields.get("ttl_ms").map(String::as_str), Some("600000"));
    assert_eq!(
        ev.fields.get("artifact.ref").map(String::as_str),
        Some("acme/api#7")
    );
    assert_eq!(
        ev.fields.get("message").map(String::as_str),
        Some("worker:  [acme/api#7] mechanical claimed lease (ttl 10m) | running mark_untriaged")
    );
}

#[test]
fn ci_completed_carries_pr_ref_and_numeric_duration() {
    let layer = CaptureLayer::default();
    let events = layer.events.clone();
    let subscriber = registry().with(layer);

    with_default(subscriber, || {
        let pr = WorkItemRef::pull_request("acme/widgets", 44);
        emit::emit_ci_completed(emit::CiCompleted {
            item: &pr,
            conclusion: "success",
            duration_ms: 280_000,
        });
    });

    let captured = events.lock().unwrap();
    let ev = &captured[0];
    assert_eq!(ev.target, "temper::trigger");
    assert_eq!(
        ev.fields.get("pr.ref").map(String::as_str),
        Some("acme/widgets PR#44")
    );
    assert_eq!(
        ev.fields.get("duration_ms").map(String::as_str),
        Some("280000")
    );
    assert_eq!(
        ev.fields.get("message").map(String::as_str),
        Some("trigger: [acme/widgets PR#44] CI completed: success (4m40s)")
    );
}

/// §7's "service prefix on every line, padded so the message column aligns":
/// the human MESSAGE carries the padded `engine:  ` / `worker:  ` / `agent:   `
/// / `trigger: ` prefix, while the structured `service=` field stays the bare
/// token. The prefix column width is identical across services so the message
/// text lines up in `journalctl`.
#[test]
fn every_emit_message_carries_the_padded_service_prefix() {
    let layer = CaptureLayer::default();
    let events = layer.events.clone();
    let subscriber = registry().with(layer);

    with_default(subscriber, || {
        let issue = WorkItemRef::issue("acme/widgets", 42);
        let pr = WorkItemRef::pull_request("acme/widgets", 44);
        // One event per plane, plus a startup-status line.
        emit::emit_transition_applied(TransitionApplied {
            item: &issue,
            transition: "mark_untriaged",
            detail: "",
            labels_delta: "+untriaged",
        });
        emit::emit_lease_released(emit::LeaseReleased {
            item: &issue,
            role: "architect",
        });
        emit::emit_agent_started(emit::AgentStarted {
            item: &issue,
            role: "architect",
            kind: "triage",
            detail: "reading issue + repo context",
        });
        emit::emit_ci_completed(emit::CiCompleted {
            item: &pr,
            conclusion: "success",
            duration_ms: 280_000,
        });
        emit::emit_engine_status("ready -- watching acme/widgets, idle");
    });

    let captured = events.lock().unwrap();
    let message_of = |service: &str| -> String {
        captured
            .iter()
            .find(|ev| ev.fields.get("service").map(String::as_str) == Some(service))
            .and_then(|ev| ev.fields.get("message").cloned())
            .unwrap_or_else(|| panic!("no captured event for service {service}"))
    };

    // The exact §7 lines, prefix included, for each plane.
    assert_eq!(
        message_of("engine"),
        "engine:  [acme/widgets#42] mark_untriaged applied | +untriaged"
    );
    assert_eq!(
        message_of("worker"),
        "worker:  [acme/widgets#42] architect lease released"
    );
    assert_eq!(
        message_of("agent"),
        "agent:   [acme/widgets#42] architect/triage start | reading issue + repo context"
    );
    assert_eq!(
        message_of("trigger"),
        "trigger: [acme/widgets PR#44] CI completed: success (4m40s)"
    );

    // Every message starts with `<service>:` and the prefix column is the same
    // width across planes (the alignment rule), while `service=` is the bare
    // token (the prefix never leaks into the machine field).
    let prefix_width = "trigger: ".len();
    for ev in captured.iter() {
        let service = ev.fields.get("service").map(String::as_str).unwrap();
        let message = ev.fields.get("message").unwrap();
        assert!(
            message.starts_with(&format!("{service}:")),
            "message for {service} must start with its prefix: {message:?}"
        );
        assert_eq!(
            &message[..prefix_width],
            &format!("{service}:{}", " ".repeat(prefix_width - service.len() - 1)),
            "{service} prefix is not padded to the aligned column width"
        );
    }
}
