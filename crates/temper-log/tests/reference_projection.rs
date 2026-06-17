// SPDX-License-Identifier: MPL-2.0

//! Regression coverage for the §7 reference projection: every emitted human
//! `message` includes the padded service prefix while the machine projection
//! remains real tracing fields with numeric measurements kept numeric.

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use temper_log::emit::{
    self, AgentFinished, AgentStarted, CiCompleted, GateEvaluated, IssueOpened, ItemResolved,
    LeaseClaimed, LeaseReleased, PrMerged, PrOpened, QueueEntered, RoleSaturated,
    TransitionApplied, WakeReceived,
};
use temper_log::{Event, WorkItemRef, work_item_span};
use tracing::field::{Field, Visit};
use tracing::subscriber::with_default;
use tracing_subscriber::Layer;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry;

/// Captured field values keep their tracing type when the visitor receives one.
#[derive(Clone, Debug, PartialEq)]
enum FieldValue {
    Text(String),
    U64(u64),
    I64(i64),
    Bool(bool),
    Debug(String),
}

impl FieldValue {
    fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(value) | Self::Debug(value) => Some(value.as_str()),
            Self::U64(_) | Self::I64(_) | Self::Bool(_) => None,
        }
    }

    fn as_u64(&self) -> Option<u64> {
        match self {
            Self::U64(value) => Some(*value),
            Self::Text(_) | Self::I64(_) | Self::Bool(_) | Self::Debug(_) => None,
        }
    }
}

/// One captured event: its target plus all field name → typed value pairs.
#[derive(Clone, Debug, Default)]
struct Captured {
    target: String,
    fields: BTreeMap<String, FieldValue>,
}

impl Captured {
    fn field(&self, key: &str) -> &FieldValue {
        self.fields
            .get(key)
            .unwrap_or_else(|| panic!("missing field {key:?} on {self:?}"))
    }

    fn text(&self, key: &str) -> &str {
        self.field(key)
            .as_text()
            .unwrap_or_else(|| panic!("field {key:?} is not text on {self:?}"))
    }

    fn text_opt(&self, key: &str) -> Option<&str> {
        self.fields.get(key).and_then(FieldValue::as_text)
    }

    fn u64(&self, key: &str) -> u64 {
        self.field(key)
            .as_u64()
            .unwrap_or_else(|| panic!("field {key:?} is not u64 on {self:?}"))
    }
}

#[derive(Default)]
struct Visitor {
    fields: BTreeMap<String, FieldValue>,
}

impl Visit for Visitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields.insert(
            field.name().to_string(),
            FieldValue::Debug(format!("{value:?}")),
        );
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields.insert(
            field.name().to_string(),
            FieldValue::Text(value.to_string()),
        );
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), FieldValue::U64(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), FieldValue::I64(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), FieldValue::Bool(value));
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
fn section_seven_reference_projection_is_prefixed_and_structured() {
    let layer = CaptureLayer::default();
    let events = layer.events.clone();
    let subscriber = registry().with(layer);

    with_default(subscriber, emit_reference_lifecycle);

    let captured = events.lock().unwrap();
    let messages: Vec<&str> = captured.iter().map(|event| event.text("message")).collect();
    assert_eq!(
        messages,
        vec![
            "engine:  ready -- watching acme/widgets, acme/api, idle",
            "worker:  capacity: architect=1 engineer=1 mechanical=1 (per-role, shared across all repos)",
            "trigger: webhook listener up on :8080/forgejo/webhook (issue, PR, CI events)",
            "trigger: [acme/widgets#42] issue opened by alice \"Cache invalidation drops stale keys on resize\"",
            "trigger: [acme/widgets#42] wake | artifact=intake queue=raw_intake",
            "worker:  [acme/widgets#42] architect claimed lease (ttl 10m) | running triage_intake",
            "agent:   [acme/widgets#42] architect/triage start | reading issue + repo context",
            "worker:  architect busy (concurrency=1) | 2 queued: acme/api#7, acme/widgets#43",
            "agent:   [acme/widgets#42] architect/triage done in 1m13s | verdict=ready_code",
            "engine:  [acme/widgets#42] triage_intake_to_code applied | body rewritten as code spec | -untriaged +code +ready",
            "engine:  [acme/widgets#42] -> queue 'code_ready' | awaiting engineer",
            "worker:  [acme/widgets#42] architect lease released",
            "engine:  [acme/widgets PR#44] opened \"Fix cache invalidation on resize\" | implementation, for #42",
            "engine:  [acme/widgets PR#44] gates: ci_gate=pending dependency_gate=ok | waiting on CI",
            "trigger: [acme/widgets PR#44] CI completed: success (4m40s)",
            "engine:  [acme/widgets PR#44] gates: ci_gate=ok dependency_gate=ok | -> queue 'landing' eligible to land",
            "engine:  [acme/widgets PR#44] merged -> main (squash e3f9a1c) | +landed",
            "engine:  [acme/widgets#42] resolved -- implemented by PR#44 | intake -> landed in 9m19s",
        ]
    );

    assert_status_event(&captured[0], "temper::engine", "engine");
    assert_status_event(&captured[1], "temper::worker", "worker");
    assert_status_event(&captured[2], "temper::trigger", "trigger");

    let issue_opened = event_named(&captured, Event::IssueOpened.as_str());
    assert_eq!(issue_opened.target, "temper::trigger");
    assert_eq!(issue_opened.text("service"), "trigger");
    assert_eq!(issue_opened.text("repo"), "acme/widgets");
    assert_eq!(issue_opened.text("artifact.ref"), "acme/widgets#42");
    assert_eq!(issue_opened.text("artifact.kind"), "issue");

    let wake = event_named(&captured, Event::WakeReceived.as_str());
    assert_eq!(wake.text("queue.to"), "raw_intake");
    assert_eq!(wake.text("artifact.ref"), "acme/widgets#42");

    let lease_claimed = event_named(&captured, Event::LeaseClaimed.as_str());
    assert_eq!(lease_claimed.target, "temper::worker");
    assert_eq!(lease_claimed.text("service"), "worker");
    assert_eq!(lease_claimed.u64("ttl_ms"), 600_000);
    assert!(lease_claimed.text("message").contains("ttl 10m"));

    let agent_finished = event_named(&captured, Event::AgentFinished.as_str());
    assert_eq!(agent_finished.target, "temper::agent");
    assert_eq!(agent_finished.text("service"), "agent");
    assert_eq!(agent_finished.u64("duration_ms"), 73_000);
    assert!(agent_finished.text("message").contains("1m13s"));

    let transition = event_named(&captured, Event::TransitionApplied.as_str());
    assert_eq!(transition.target, "temper::engine");
    assert_eq!(transition.text("service"), "engine");
    assert_eq!(transition.text("transition"), "triage_intake_to_code");
    assert_eq!(transition.text("labels.delta"), "-untriaged +code +ready");

    let queue = event_named(&captured, Event::QueueEntered.as_str());
    assert_eq!(queue.text("queue.to"), "code_ready");

    let role_saturated = event_named(&captured, Event::RoleSaturated.as_str());
    assert_eq!(role_saturated.text("service"), "worker");
    assert_eq!(role_saturated.text("role"), "architect");
    assert_eq!(role_saturated.u64("concurrency"), 1);
    assert_eq!(role_saturated.text("queued"), "acme/api#7,acme/widgets#43");

    let pr_opened = event_named(&captured, Event::PrOpened.as_str());
    assert_eq!(pr_opened.text("pr.ref"), "acme/widgets PR#44");
    assert_eq!(pr_opened.text("artifact.ref"), "acme/widgets PR#44");
    assert_eq!(pr_opened.u64("for_issue"), 42);

    let gate = event_named(&captured, Event::GateEvaluated.as_str());
    assert_eq!(gate.text("pr.ref"), "acme/widgets PR#44");
    assert_eq!(gate.text("gates"), "ci_gate=pending dependency_gate=ok");

    let ci = event_named(&captured, Event::CiCompleted.as_str());
    assert_eq!(ci.target, "temper::trigger");
    assert_eq!(ci.text("service"), "trigger");
    assert_eq!(ci.text("pr.ref"), "acme/widgets PR#44");
    assert_eq!(ci.u64("duration_ms"), 280_000);
    assert!(ci.text("message").contains("4m40s"));

    let merged = event_named(&captured, Event::PrMerged.as_str());
    assert_eq!(merged.text("pr.ref"), "acme/widgets PR#44");
    assert_eq!(merged.text("labels.delta"), "+landed");

    let resolved = event_named(&captured, Event::ItemResolved.as_str());
    assert_eq!(resolved.text("artifact.ref"), "acme/widgets#42");
    assert_eq!(resolved.u64("duration_ms"), 559_000);
    assert!(resolved.text("message").contains("9m19s"));

    for event in captured
        .iter()
        .filter(|event| event.fields.contains_key("artifact.ref"))
    {
        assert_artifact_ref_matches_human_tag(event);
    }
}

#[test]
fn json_fmt_projection_keeps_prefixed_message_and_queryable_fields() {
    let buf = SharedBuf::default();
    let sink = buf.clone();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_current_span(true)
        .with_span_list(true)
        .with_writer(sink)
        .with_max_level(tracing::Level::TRACE)
        .finish();

    with_default(subscriber, || {
        let item = WorkItemRef::issue("acme/widgets", 42);
        let span = work_item_span(&item, "architect", Some("triage_intake"));
        let _guard = span.enter();
        emit::emit_agent_finished(AgentFinished {
            item: &item,
            role: "architect",
            kind: "triage",
            duration_ms: 73_000,
            summary: "verdict=ready_code",
        });
    });

    let rendered = String::from_utf8(buf.bytes()).expect("utf8 json log output");
    let records: Vec<Value> = rendered
        .lines()
        .map(|line| serde_json::from_str(line).expect("json log line parses"))
        .collect();
    let event = records
        .iter()
        .find(|record| {
            record.pointer("/fields/event").and_then(Value::as_str) == Some("agent.finished")
        })
        .expect("agent.finished event rendered as JSON");
    let fields = event
        .get("fields")
        .and_then(Value::as_object)
        .expect("JSON event has fields object");

    assert_eq!(
        fields.get("message").and_then(Value::as_str),
        Some("agent:   [acme/widgets#42] architect/triage done in 1m13s | verdict=ready_code")
    );
    assert_eq!(fields.get("service").and_then(Value::as_str), Some("agent"));
    assert_eq!(
        fields.get("artifact.ref").and_then(Value::as_str),
        Some("acme/widgets#42")
    );
    assert_eq!(
        fields.get("duration_ms").and_then(Value::as_u64),
        Some(73_000)
    );

    let span = event
        .get("span")
        .and_then(Value::as_object)
        .expect("current work_item span is rendered");
    assert_eq!(span.get("name").and_then(Value::as_str), Some("work_item"));
    assert_eq!(
        span.get("artifact.ref").and_then(Value::as_str),
        Some("acme/widgets#42")
    );
}

fn emit_reference_lifecycle() {
    let issue = WorkItemRef::issue("acme/widgets", 42);
    let api_issue = WorkItemRef::issue("acme/api", 7);
    let queued_issue = WorkItemRef::issue("acme/widgets", 43);
    let pr = WorkItemRef::pull_request("acme/widgets", 44);
    let waiting = vec![api_issue.to_string(), queued_issue.to_string()];

    emit::emit_engine_status("ready -- watching acme/widgets, acme/api, idle");
    emit::emit_worker_status(
        "capacity: architect=1 engineer=1 mechanical=1 (per-role, shared across all repos)",
    );
    emit::emit_trigger_status(
        "webhook listener up on :8080/forgejo/webhook (issue, PR, CI events)",
    );
    emit::emit_issue_opened(IssueOpened {
        item: &issue,
        author: "alice",
        title: "Cache invalidation drops stale keys on resize",
    });
    emit::emit_wake_received(WakeReceived {
        item: &issue,
        artifact_kind: "intake",
        queue: "raw_intake",
    });
    emit::emit_lease_claimed(LeaseClaimed {
        item: &issue,
        role: "architect",
        ttl_human: "10m",
        ttl_ms: 600_000,
        running: "triage_intake",
    });
    emit::emit_agent_started(AgentStarted {
        item: &issue,
        role: "architect",
        kind: "triage",
        detail: "reading issue + repo context",
    });
    emit::emit_role_saturated(RoleSaturated {
        role: "architect",
        concurrency: 1,
        waiting: &waiting,
    });
    emit::emit_agent_finished(AgentFinished {
        item: &issue,
        role: "architect",
        kind: "triage",
        duration_ms: 73_000,
        summary: "verdict=ready_code",
    });
    emit::emit_transition_applied(TransitionApplied {
        item: &issue,
        transition: "triage_intake_to_code",
        detail: "body rewritten as code spec",
        labels_delta: "-untriaged +code +ready",
    });
    emit::emit_queue_entered(QueueEntered {
        item: &issue,
        queue_to: "code_ready",
        note: "awaiting engineer",
    });
    emit::emit_lease_released(LeaseReleased {
        item: &issue,
        role: "architect",
    });
    emit::emit_pr_opened(PrOpened {
        item: &pr,
        title: "Fix cache invalidation on resize",
        kind: "implementation",
        for_issue: 42,
    });
    emit::emit_gate_evaluated(GateEvaluated {
        item: &pr,
        gates: "ci_gate=pending dependency_gate=ok",
        note: "waiting on CI",
    });
    emit::emit_ci_completed(CiCompleted {
        item: &pr,
        conclusion: "success",
        duration_ms: 280_000,
    });
    emit::emit_gate_evaluated(GateEvaluated {
        item: &pr,
        gates: "ci_gate=ok dependency_gate=ok",
        note: "-> queue 'landing' eligible to land",
    });
    emit::emit_pr_merged(PrMerged {
        item: &pr,
        target_branch: "main",
        merge_method: "squash",
        commit: "e3f9a1c",
        labels_delta: "+landed",
    });
    emit::emit_item_resolved(ItemResolved {
        item: &issue,
        resolution: "implemented by PR#44",
        from_state: "intake",
        to_state: "landed",
        duration_ms: 559_000,
    });
}

fn assert_status_event(event: &Captured, target: &str, service: &str) {
    assert_eq!(event.target, target);
    assert_eq!(event.text("service"), service);
    assert!(
        event.text_opt("event").is_none(),
        "status lines do not use the closed event vocabulary: {event:?}"
    );
}

fn event_named<'a>(events: &'a [Captured], name: &str) -> &'a Captured {
    let event = events
        .iter()
        .find(|event| event.text_opt("event") == Some(name))
        .unwrap_or_else(|| panic!("missing event {name}"));
    assert_eq!(event.text("event"), name);
    event
}

fn assert_artifact_ref_matches_human_tag(event: &Captured) {
    let artifact_ref = event.text("artifact.ref");
    let message = event.text("message");
    let tag_start = message
        .find('[')
        .unwrap_or_else(|| panic!("message has no human tag: {message}"));
    let after_start = tag_start + '['.len_utf8();
    let tag_end = message[after_start..]
        .find(']')
        .map(|offset| after_start + offset)
        .unwrap_or_else(|| panic!("message has no closing human tag: {message}"));
    assert_eq!(&message[after_start..tag_end], artifact_ref);
}

/// A `MakeWriter` over a shared buffer, so tests can parse JSON fmt output.
#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl SharedBuf {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

impl io::Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedBuf {
    type Writer = SharedBuf;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
