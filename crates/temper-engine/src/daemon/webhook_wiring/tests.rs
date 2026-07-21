// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use temper_forge::ItemNumber;
use tracing::field::{Field, Visit};
use tracing::subscriber::with_default;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry;

use super::*;
use crate::daemon::wake_coordinator::CiTriggerSource;

#[derive(Clone, Default)]
struct RecordingMechanical {
    events: Arc<std::sync::Mutex<Vec<String>>>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    changed: bool,
}

impl RecordingMechanical {
    async fn record(&self, event: String) {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        self.events.lock().expect("event log").push(event);
        let mut yielded = false;
        std::future::poll_fn(move |cx| {
            if yielded {
                std::task::Poll::Ready(())
            } else {
                yielded = true;
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
        })
        .await;
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

impl CoordinatedMechanical for RecordingMechanical {
    fn run_coordinated_broad(
        &self,
        _repo: RepositoryPath,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send>> {
        let recorder = self.clone();
        Box::pin(async move {
            recorder.record("mechanical:broad".to_string()).await;
            Ok(recorder.changed)
        })
    }

    fn run_coordinated_targeted(
        &self,
        _repo: RepositoryPath,
        artifact: ArtifactAddress,
        change: ChangeKind,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send>> {
        let recorder = self.clone();
        Box::pin(async move {
            recorder
                .record(format!(
                    "mechanical:{}#{}:{change:?}",
                    artifact_kind(artifact.kind),
                    artifact.number
                ))
                .await;
            Ok(recorder.changed)
        })
    }
}

#[test]
fn executor_serializes_priority_targets_then_broad_then_role_work() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let recorder = RecordingMechanical::default();
        let events = Arc::clone(&recorder.events);
        let max_active = Arc::clone(&recorder.max_active);
        let mechanical: Arc<dyn CoordinatedMechanical> = Arc::new(recorder);
        let mut targets = WakeTargets::new();
        targets.insert(
            (HintArtifactKind::Issue, ItemNumber::new(2)),
            WakeTarget::new(ChangeKind::Label, None),
        );
        targets.insert(
            (HintArtifactKind::PullRequest, ItemNumber::new(8)),
            WakeTarget::new(ChangeKind::Edited, None),
        );
        targets.insert(
            (HintArtifactKind::PullRequest, ItemNumber::new(9)),
            WakeTarget::new(ChangeKind::Ci, None),
        );
        let mut failures = Vec::new();

        let changed = execute_mechanical_work(
            Some(&mechanical),
            &RepositoryPath::new("ai", "temper"),
            &targets,
            true,
            &mut failures,
        )
        .await;
        events
            .lock()
            .expect("event log")
            .push("role:scan".to_string());

        assert!(failures.is_empty());
        assert!(!changed);
        assert_eq!(
            *events.lock().expect("event log"),
            vec![
                "mechanical:pull_request#9:Ci",
                "mechanical:pull_request#8:Edited",
                "mechanical:issue#2:Label",
                "mechanical:broad",
                "role:scan",
            ]
        );
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn mechanical_change_is_reported_for_role_followup() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let recorder = RecordingMechanical {
            changed: true,
            ..RecordingMechanical::default()
        };
        let mechanical: Arc<dyn CoordinatedMechanical> = Arc::new(recorder);
        let mut failures = Vec::new();

        let changed = execute_mechanical_work(
            Some(&mechanical),
            &RepositoryPath::new("ai", "temper"),
            &WakeTargets::new(),
            true,
            &mut failures,
        )
        .await;

        assert!(failures.is_empty());
        assert!(changed);
    });
}

#[derive(Clone, Debug, Default)]
struct CapturedCiEvent {
    fields: BTreeMap<String, String>,
}

#[derive(Default)]
struct CiEventVisitor {
    fields: BTreeMap<String, String>,
}

impl Visit for CiEventVisitor {
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
struct CiCaptureLayer {
    events: Arc<std::sync::Mutex<Vec<CapturedCiEvent>>>,
}

impl<S: tracing::Subscriber> Layer<S> for CiCaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _context: Context<'_, S>) {
        if event.metadata().target() != "temper::trigger" {
            return;
        }
        let mut visitor = CiEventVisitor::default();
        event.record(&mut visitor);
        self.events
            .lock()
            .expect("CI event capture")
            .push(CapturedCiEvent {
                fields: visitor.fields,
            });
    }
}

#[test]
fn coordinated_ci_observability_preserves_webhook_and_poll_provenance() {
    let layer = CiCaptureLayer::default();
    let events = Arc::clone(&layer.events);
    let subscriber = registry().with(layer);
    let repository = RepositoryPath::new("ai", "temper");
    let address = ArtifactAddress::pull_request(ItemNumber::new(627));
    let selected = (
        temper_workflow::QueueId::new("pr_ci_failed"),
        temper_workflow::RoleId::new("engineer"),
    );
    let detected_at: chrono::DateTime<chrono::Utc> = "2026-07-21T12:00:01Z".parse().unwrap();

    with_default(subscriber, || {
        for (source, completed_at) in [
            (CiTriggerSource::Webhook, "2026-07-21T12:00:02Z"),
            (CiTriggerSource::CiPoll, "2026-07-21T11:59:59.500Z"),
        ] {
            emit_ci_wake_observation(
                &repository,
                address,
                CiWakeFacts {
                    source,
                    verdict: Some(crate::CiTerminalVerdict::Failed),
                    completed_at: Some(completed_at.parse().unwrap()),
                },
                Some(CiStatus::failed()),
                Some(&selected),
                detected_at,
            );
        }
    });

    let captured = events.lock().expect("CI event capture");
    assert_eq!(captured.len(), 2);
    for (event, source, latency) in [
        (&captured[0], "webhook", "0"),
        (&captured[1], "ci_poll", "1500"),
    ] {
        assert_eq!(
            event.fields.get("pr.ref").map(String::as_str),
            Some("ai/temper PR#627")
        );
        assert_eq!(
            event.fields.get("conclusion").map(String::as_str),
            Some("failure")
        );
        assert_eq!(
            event.fields.get("trigger.source").map(String::as_str),
            Some(source)
        );
        assert_eq!(
            event
                .fields
                .get("ci.detection_latency_ms")
                .map(String::as_str),
            Some(latency)
        );
        assert_eq!(
            event.fields.get("queue").map(String::as_str),
            Some("pr_ci_failed")
        );
        assert_eq!(
            event.fields.get("role").map(String::as_str),
            Some("engineer")
        );
    }
}
