use super::super::lifecycle_observability::{
    DiscoveryEvidence, DiscoveryOutcome, FailureCategory, IndexOutcome, ReadinessOutcome,
    emit_discovery, emit_identity_selected, emit_index, emit_readiness,
};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::field::{Field, Visit};
use tracing::subscriber::with_default;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};

#[derive(Clone, Debug)]
struct CapturedEvent {
    level: Level,
    fields: BTreeMap<String, String>,
}

#[derive(Clone)]
struct CaptureLayer(Arc<Mutex<Vec<CapturedEvent>>>);

#[derive(Default)]
struct FieldVisitor(BTreeMap<String, String>);

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

impl<S: Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        if event.metadata().target() != "temper::agent" {
            return;
        }
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        self.0.lock().unwrap().push(CapturedEvent {
            level: *event.metadata().level(),
            fields: visitor.0,
        });
    }
}

fn capture(run: impl FnOnce()) -> Vec<CapturedEvent> {
    let events = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(CaptureLayer(Arc::clone(&events)));
    with_default(subscriber, run);
    Arc::try_unwrap(events).unwrap().into_inner().unwrap()
}

fn event<'a>(
    events: &'a [CapturedEvent],
    name: &str,
    outcome_field: &str,
    outcome: &str,
) -> &'a CapturedEvent {
    events
        .iter()
        .find(|event| {
            event.fields.get("event").map(String::as_str) == Some(name)
                && event.fields.get(outcome_field).map(String::as_str) == Some(outcome)
        })
        .unwrap_or_else(|| panic!("missing {name} {outcome_field}={outcome}: {events:#?}"))
}

#[test]
fn lifecycle_events_have_exact_safe_fields_levels_and_unavailable_bytes() {
    let secret_path = "/srv/data/private/token=do-not-log/repository";
    let events = capture(|| {
        emit_discovery(DiscoveryEvidence {
            method: "index_status",
            inventory: "targeted",
            duration: Duration::from_millis(17),
            outcome: DiscoveryOutcome::Success,
            record_count: 2,
            cache_bytes: None,
            failure: FailureCategory::None,
        });
        emit_discovery(DiscoveryEvidence {
            method: "index_status",
            inventory: "targeted",
            duration: Duration::from_millis(51),
            outcome: DiscoveryOutcome::Timeout,
            record_count: 0,
            cache_bytes: Some(4096),
            failure: FailureCategory::Timeout,
        });
        emit_identity_selected(secret_path, "temper-v1-safe", "migrated");
        for identity_outcome in ["reused", "missing", "stale"] {
            emit_identity_selected("acme/temper", "temper-v1-safe", identity_outcome);
        }
        for index_outcome in [
            IndexOutcome::Requested,
            IndexOutcome::Started,
            IndexOutcome::Reused,
            IndexOutcome::RebindFresh,
            IndexOutcome::SuppressedDuplicate,
            IndexOutcome::Completed,
            IndexOutcome::SkippedDiscoveryUnknown,
            IndexOutcome::Disabled,
        ] {
            emit_index(
                "acme/temper",
                "temper-v1-safe",
                "background",
                index_outcome,
                FailureCategory::None,
            );
        }
        emit_index(
            "acme/temper",
            "temper-v1-safe",
            "background",
            IndexOutcome::Failed,
            FailureCategory::Provider,
        );
        emit_readiness(
            "temper-v1-safe",
            Duration::from_millis(2),
            ReadinessOutcome::Success,
            FailureCategory::None,
        );
        emit_readiness(
            "temper-v1-safe",
            Duration::from_millis(3),
            ReadinessOutcome::Failure,
            FailureCategory::Provider,
        );
        emit_readiness(
            "temper-v1-safe",
            Duration::from_millis(99),
            ReadinessOutcome::Timeout,
            FailureCategory::Timeout,
        );
    });

    let discovery = event(
        &events,
        "codebase_memory.discovery.completed",
        "outcome",
        "success",
    );
    assert_eq!(discovery.level, Level::DEBUG);
    assert_eq!(discovery.fields["discovery.method"], "index_status");
    assert_eq!(discovery.fields["discovery.inventory"], "targeted");
    assert_eq!(discovery.fields["discovery.targeted"], "true");
    assert_eq!(discovery.fields["duration_ms"], "17");
    assert_eq!(discovery.fields["record_count"], "2");
    assert_eq!(discovery.fields["cache.bytes_available"], "false");
    assert_eq!(discovery.fields["cache.bytes"], "0");

    let timeout = event(
        &events,
        "codebase_memory.discovery.completed",
        "outcome",
        "timeout",
    );
    assert_eq!(timeout.level, Level::WARN);
    assert_eq!(timeout.fields["timed_out"], "true");
    assert_eq!(timeout.fields["cache.bytes_available"], "true");
    assert_eq!(timeout.fields["cache.bytes"], "4096");
    assert_eq!(timeout.fields["failure.category"], "timeout");

    for outcome in [
        "requested",
        "started",
        "reused",
        "rebind_fresh",
        "suppressed_duplicate",
        "completed",
        "skipped_discovery_unknown",
        "disabled",
    ] {
        let index = event(
            &events,
            "codebase_memory.index.lifecycle",
            "index.outcome",
            outcome,
        );
        assert_eq!(index.level, Level::DEBUG);
    }
    let failed_index = event(
        &events,
        "codebase_memory.index.lifecycle",
        "index.outcome",
        "failed",
    );
    assert_eq!(failed_index.level, Level::WARN);
    assert_eq!(failed_index.fields["failure.category"], "provider_error");

    for outcome in ["reused", "migrated", "missing", "stale"] {
        let identity = event(
            &events,
            "codebase_memory.identity.selected",
            "identity.outcome",
            outcome,
        );
        assert_eq!(identity.level, Level::DEBUG);
    }

    let readiness_success = event(
        &events,
        "codebase_memory.readiness.wait",
        "outcome",
        "success",
    );
    assert_eq!(readiness_success.level, Level::DEBUG);
    let readiness_failure = event(
        &events,
        "codebase_memory.readiness.wait",
        "outcome",
        "failure",
    );
    assert_eq!(readiness_failure.level, Level::WARN);
    let readiness = event(
        &events,
        "codebase_memory.readiness.wait",
        "outcome",
        "timeout",
    );
    assert_eq!(readiness.level, Level::WARN);
    assert_eq!(readiness.fields["duration_ms"], "99");

    let rendered = format!("{events:#?}");
    assert!(!rendered.contains(secret_path));
    assert!(!rendered.contains("do-not-log"));
    assert!(rendered.contains("<redacted-identifier>"));
    assert!(events.iter().all(|event| {
        event
            .fields
            .get("failure.message")
            .is_none_or(|message| message.chars().count() <= 64)
    }));
}
