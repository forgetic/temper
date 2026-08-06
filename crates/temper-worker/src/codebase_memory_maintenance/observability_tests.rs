use super::*;
use crate::codebase_memory_retention::{
    CodebaseMemoryRetentionFailure, CodebaseMemoryRetentionRecordResult,
};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
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

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

impl<S: Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        if event.metadata().target() != "temper::worker" {
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

#[test]
fn retention_events_are_aggregate_safe_and_show_partial_failures() {
    let sensitive = "/srv/data/private/token=do-not-log/temper";
    let record = CodebaseMemoryRetentionRecordResult {
        project: sensitive.to_string(),
        repo_path: Some(PathBuf::from(sensitive)),
        estimated_bytes: Some(2048),
        reason: "operator detail".to_string(),
    };
    let report = CodebaseMemoryRetentionReport {
        cache_instance_id: Some("cache-private".to_string()),
        cache_bytes: None,
        inventory_complete: true,
        inventory_attempted: true,
        inventory_record_count: 7,
        inventory_duration_ms: 45,
        no_op_reason: None,
        outcome: CodebaseMemoryRetentionOutcome::PartialFailure,
        policy: Some(CodebaseMemoryRetentionPolicy {
            enabled: true,
            max_obsolete_projects: 64,
            max_age_days: 30,
            maintenance_interval_secs: 3600,
            maintenance_timeout_secs: 30,
            inventory_page_size: 50,
            max_inventory_pages: 20,
            max_deletions_per_run: 16,
        }),
        duration_ms: 123,
        dry_run: true,
        deleted_estimated_bytes: Some(2048),
        preserved: vec![record.clone(); 2],
        candidates: vec![record.clone(); 3],
        proposed: Vec::new(),
        deleted: vec![record.clone()],
        failed: vec![CodebaseMemoryRetentionFailure {
            record,
            error: "Bearer credential and absolute path must stay explicit-only".to_string(),
        }],
    };

    let events = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(CaptureLayer(Arc::clone(&events)));
    with_default(subscriber, || emit_report(&report));
    let events = Arc::try_unwrap(events).unwrap().into_inner().unwrap();

    let discovery = events
        .iter()
        .find(|event| {
            event.fields.get("event").map(String::as_str)
                == Some("codebase_memory.maintenance.discovery.completed")
        })
        .expect("maintenance discovery event");
    assert_eq!(discovery.level, Level::DEBUG);
    assert_eq!(discovery.fields["discovery.inventory"], "maintenance");
    assert_eq!(discovery.fields["discovery.targeted"], "false");
    assert_eq!(discovery.fields["record_count"], "7");
    assert_eq!(discovery.fields["duration_ms"], "45");
    assert_eq!(discovery.fields["cache.bytes_available"], "false");
    assert_eq!(discovery.fields["cache.bytes"], "0");

    let retention = events
        .iter()
        .find(|event| {
            event.fields.get("event").map(String::as_str)
                == Some("codebase_memory.retention.completed")
        })
        .expect("retention event");
    assert_eq!(retention.level, Level::WARN);
    assert_eq!(retention.fields["outcome"], "partial_failure");
    assert_eq!(retention.fields["duration_ms"], "123");
    assert_eq!(retention.fields["retention.enabled"], "true");
    assert_eq!(retention.fields["retention.max_obsolete_projects"], "64");
    assert_eq!(retention.fields["retention.max_age_days"], "30");
    assert_eq!(retention.fields["retention.max_deletions_per_run"], "16");
    assert_eq!(retention.fields["retention.preserved_count"], "2");
    assert_eq!(retention.fields["retention.candidate_count"], "3");
    assert_eq!(retention.fields["retention.deletion_count"], "1");
    assert_eq!(
        retention.fields["retention.deleted_bytes_available"],
        "true"
    );
    assert_eq!(
        retention.fields["retention.deleted_estimated_bytes"],
        "2048"
    );
    assert_eq!(retention.fields["retention.dry_run"], "true");
    assert_eq!(retention.fields["failure.count"], "1");
    assert_eq!(retention.fields["failure.category"], "deletion_failure");

    let rendered = format!("{events:#?}");
    assert!(!rendered.contains(sensitive));
    assert!(!rendered.contains("credential"));
    assert!(!rendered.contains("cache-private"));
    assert!(!rendered.contains("operator detail"));
}
