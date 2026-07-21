// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Utc};
use temper_forge_memory::FaultOp;
use temper_workflow::{RecoveredHeartbeatOutcome, RecoveredOwnershipLossReason, WorkflowMetadata};
use tracing::Level;
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::{Layer, registry};

use super::*;

#[derive(Clone, Debug)]
struct CapturedEvent {
    level: Level,
    target: String,
    fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(|value| value.trim_matches('"'))
    }
}

#[derive(Default)]
struct CapturedVisitor {
    fields: BTreeMap<String, String>,
}

impl Visit for CapturedVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

#[derive(Clone)]
struct CaptureLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _context: Context<'_, S>) {
        let mut visitor = CapturedVisitor::default();
        event.record(&mut visitor);
        self.events
            .lock()
            .expect("capture lock")
            .push(CapturedEvent {
                level: *event.metadata().level(),
                target: event.metadata().target().to_string(),
                fields: visitor.fields,
            });
    }
}

fn event_store() -> &'static Arc<Mutex<Vec<CapturedEvent>>> {
    static EVENTS: OnceLock<Arc<Mutex<Vec<CapturedEvent>>>> = OnceLock::new();
    EVENTS.get_or_init(|| {
        let events = Arc::new(Mutex::new(Vec::new()));
        tracing::subscriber::set_global_default(registry().with(CaptureLayer {
            events: Arc::clone(&events),
        }))
        .expect("install lease diagnostic capture subscriber");
        events
    })
}

fn capture_events(run: impl FnOnce()) -> Vec<CapturedEvent> {
    let events = event_store();
    run();
    let captured = events.lock().expect("capture lock").clone();
    captured
}

fn diagnostic<'a>(
    events: &'a [CapturedEvent],
    level: Level,
    operation: &str,
    message: &str,
    expected_field: (&str, &str),
) -> &'a CapturedEvent {
    events
        .iter()
        .find(|event| {
            event.level == level
                && event.target == "temper_daemon"
                && event.field("operation") == Some(operation)
                && event.field("message") == Some(message)
                && event
                    .field(expected_field.0)
                    .is_some_and(|value| value.contains(expected_field.1))
        })
        .unwrap_or_else(|| {
            panic!(
                "missing {level} operation={operation} diagnostic {message:?} with {} containing {:?} in {events:#?}",
                expected_field.0, expected_field.1
            )
        })
}

fn claim_context() -> temper_engine::ClaimContext {
    temper_engine::ClaimContext {
        worker_id: "worker-a".to_string(),
        daemon_boot_id: "daemon-1".to_string(),
    }
}

fn recording_inner() -> (
    Arc<RecordingApplier>,
    temper_engine_io::CqReceiver<(InFlightJob, JobResult)>,
) {
    let (tx, rx) = temper_engine_io::channel();
    (
        Arc::new(RecordingApplier {
            tx,
            forge: None,
            repo: None,
            issue: None,
            lease_tx: None,
            freshness_response: None,
        }),
        rx,
    )
}

async fn issue_metadata(
    forge: &MemoryForge,
    repo: &RepositoryId,
    issue: ItemNumber,
) -> WorkflowMetadata {
    let issue = forge
        .get_issue_by_number(repo, issue)
        .await
        .expect("issue reload succeeds")
        .expect("issue exists");
    parse_metadata_block(&issue.body)
        .expect("issue metadata parses")
        .unwrap_or_default()
}

fn controlled_clock(
    initial: DateTime<Utc>,
) -> (Arc<Mutex<DateTime<Utc>>>, temper_engine::WallClock) {
    let now = Arc::new(Mutex::new(initial));
    let captured = Arc::clone(&now);
    (now, Arc::new(move || *captured.lock().expect("clock lock")))
}

#[test]
fn repository_lookup_failure_is_retryable_while_missing_and_malformed_targets_are_stale() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let (inner, mut rx) = recording_inner();
        let applier = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            inner,
            temper_engine::system_clock(),
        );

        forge.fail_next(
            FaultOp::GetRepositoryByPath,
            "simulated claim repository lookup failure",
        );
        let outcome = applier.claim(in_flight_job(issue), claim_context()).await;
        assert!(matches!(
            outcome,
            temper_engine::ClaimOutcome::Retryable { reason }
                if reason.contains("simulated claim repository lookup failure")
        ));
        assert!(rx.try_recv().is_none());
        let metadata = issue_metadata(&forge, &repo, issue).await;
        assert!(metadata.assignment.is_none());
        assert!(metadata.lease.is_none());

        let mut missing = in_flight_job(issue);
        missing.repo = "acme/missing".to_string();
        assert!(matches!(
            applier.claim(missing, claim_context()).await,
            temper_engine::ClaimOutcome::Stale { .. }
        ));

        let mut malformed = in_flight_job(issue);
        malformed.repo = "not-a-repository-path".to_string();
        assert!(matches!(
            applier.claim(malformed, claim_context()).await,
            temper_engine::ClaimOutcome::Stale { .. }
        ));

        let mut malformed_artifact = in_flight_job(issue);
        malformed_artifact.artifact.item = json!("not-a-number");
        assert!(matches!(
            applier.claim(malformed_artifact, claim_context()).await,
            temper_engine::ClaimOutcome::Stale { .. }
        ));
    })
}

#[test]
fn apply_lookup_failure_is_retryable_and_preserves_claim_for_one_recovery_apply() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let (inner, mut rx) = recording_inner();
        let applier = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            inner,
            temper_engine::system_clock(),
        );
        let job = in_flight_job(issue);
        let result = job_result(&job.job_id);
        assert_eq!(
            applier.claim(job.clone(), claim_context()).await,
            temper_engine::ClaimOutcome::Claimed
        );

        forge.fail_next(
            FaultOp::GetRepositoryByPath,
            "simulated apply repository lookup failure",
        );
        let failed = applier.apply(job.clone(), result.clone()).await;
        assert!(matches!(
            failed,
            temper_engine::ApplyOutcome::Retryable { reason }
                if reason.contains("simulated apply repository lookup failure")
        ));
        assert!(rx.try_recv().is_none());
        let metadata = issue_metadata(&forge, &repo, issue).await;
        assert!(metadata.assignment.is_some());
        assert!(metadata.lease.is_some());

        assert_eq!(
            applier.apply(job.clone(), result.clone()).await,
            temper_engine::ApplyOutcome::Applied
        );
        assert_eq!(rx.recv().await, Some((job, result)));
        assert!(rx.try_recv().is_none());
        let metadata = issue_metadata(&forge, &repo, issue).await;
        assert!(metadata.assignment.is_none());
        assert!(metadata.lease.is_none());
    })
}

#[test]
fn heartbeat_lookup_failure_retains_context_and_later_advances_the_lease() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let (inner, mut rx) = recording_inner();
        let initial = "2026-07-15T14:50:00Z"
            .parse::<DateTime<Utc>>()
            .expect("valid initial time");
        let (now, clock) = controlled_clock(initial);
        let applier = LeaseApplier::new(forge.clone(), policy(), "daemon-1", inner, clock);
        let job = in_flight_job(issue);
        let context = claim_context();
        assert_eq!(
            applier.claim(job.clone(), context.clone()).await,
            temper_engine::ClaimOutcome::Claimed
        );
        let initial_metadata = issue_metadata(&forge, &repo, issue).await;

        *now.lock().expect("clock lock") = initial + chrono::Duration::seconds(60);
        forge.fail_next(
            FaultOp::GetRepositoryByPath,
            "simulated heartbeat repository lookup failure",
        );
        let failed = applier.heartbeat(job.clone(), context.clone()).await;
        assert!(matches!(
            failed,
            RecoveredHeartbeatOutcome::TransientlyUnavailable { reason }
                if reason.contains("simulated heartbeat repository lookup failure")
        ));
        let failed_metadata = issue_metadata(&forge, &repo, issue).await;
        assert_eq!(failed_metadata.assignment, initial_metadata.assignment);
        assert_eq!(failed_metadata.lease, initial_metadata.lease);

        forge.fail_next(
            FaultOp::GetRepositoryByPath,
            "probe retained process-local heartbeat context",
        );
        assert!(matches!(
            applier.apply(job.clone(), job_result(&job.job_id)).await,
            temper_engine::ApplyOutcome::Retryable { reason }
                if reason.contains("probe retained process-local heartbeat context")
        ));
        assert!(rx.try_recv().is_none());

        *now.lock().expect("clock lock") = initial + chrono::Duration::seconds(120);
        assert_eq!(
            applier.heartbeat(job, context).await,
            RecoveredHeartbeatOutcome::Owned
        );
        let refreshed = issue_metadata(&forge, &repo, issue).await;
        assert!(
            refreshed
                .lease
                .as_ref()
                .expect("refreshed lease")
                .expires_at
                > initial_metadata
                    .lease
                    .as_ref()
                    .expect("initial lease")
                    .expires_at
        );
        assert!(
            refreshed
                .assignment
                .as_ref()
                .expect("refreshed assignment")
                .expires_at
                > initial_metadata
                    .assignment
                    .as_ref()
                    .expect("initial assignment")
                    .expires_at
        );
    })
}

#[test]
fn release_lookup_failure_drops_local_context_but_preserves_durable_assignment() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let (inner, mut rx) = recording_inner();
        let applier = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            inner,
            temper_engine::system_clock(),
        );
        let job = in_flight_job(issue);
        let context = claim_context();
        assert_eq!(
            applier.claim(job.clone(), context.clone()).await,
            temper_engine::ClaimOutcome::Claimed
        );
        let claimed = issue_metadata(&forge, &repo, issue).await;

        forge.fail_next(
            FaultOp::GetRepositoryByPath,
            "simulated release repository lookup failure",
        );
        applier.release_claim(job.clone(), context).await;
        let after_release = issue_metadata(&forge, &repo, issue).await;
        assert_eq!(after_release.assignment, claimed.assignment);
        assert_eq!(after_release.lease, claimed.lease);

        forge.fail_next(
            FaultOp::GetRepositoryByPath,
            "must remain unconsumed when local context was removed",
        );
        assert_eq!(
            applier.apply(job.clone(), job_result(&job.job_id)).await,
            temper_engine::ApplyOutcome::Stale
        );
        assert!(rx.try_recv().is_none());
    })
}

#[test]
fn recovered_apply_lookup_failure_retries_then_applies_once() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let issue = create_ready_issue(&forge, &repo).await;
        let job = in_flight_job(issue);
        let result = job_result(&job.job_id);
        let context = claim_context();
        let (initial_inner, _initial_rx) = recording_inner();
        let initial = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            initial_inner,
            temper_engine::system_clock(),
        );
        assert_eq!(
            initial.claim(job.clone(), context.clone()).await,
            temper_engine::ClaimOutcome::Claimed
        );
        drop(initial);

        let (recovered_inner, mut rx) = recording_inner();
        let recovered = LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-2",
            recovered_inner,
            temper_engine::system_clock(),
        );
        forge.fail_next(
            FaultOp::GetRepositoryByPath,
            "simulated recovered apply repository lookup failure",
        );
        let failed = recovered
            .apply_recovered(job.clone(), result.clone(), context.clone())
            .await;
        assert!(matches!(
            failed,
            temper_engine::ApplyOutcome::Retryable { reason }
                if reason.contains("simulated recovered apply repository lookup failure")
        ));
        assert!(rx.try_recv().is_none());
        let retained = issue_metadata(&forge, &repo, issue).await;
        assert!(retained.assignment.is_some());
        assert!(retained.lease.is_some());

        assert_eq!(
            recovered
                .apply_recovered(job.clone(), result.clone(), context)
                .await,
            temper_engine::ApplyOutcome::Applied
        );
        assert_eq!(rx.recv().await, Some((job, result)));
        assert!(rx.try_recv().is_none());
    })
}

#[test]
fn apply_missing_repository_is_stale_without_invoking_inner() {
    let events = capture_events(|| {
        temper_engine_io::block_on_with(move |_cx, _handle| async move {
            let forge = Arc::new(MemoryForge::new());
            let repo = new_repo(&forge).await;
            let issue = create_ready_issue(&forge, &repo).await;
            let (inner, mut rx) = recording_inner();
            let applier = LeaseApplier::new(
                forge.clone(),
                policy(),
                "daemon-1",
                inner,
                temper_engine::system_clock(),
            );
            let job = in_flight_job(issue);
            let result = job_result(&job.job_id);
            assert_eq!(
                applier.claim(job.clone(), claim_context()).await,
                temper_engine::ClaimOutcome::Claimed
            );
            let claimed = issue_metadata(&forge, &repo, issue).await;

            forge.fail_next(
                FaultOp::GetRepositoryByPath,
                "diagnostic apply repository lookup failure",
            );
            assert!(matches!(
                applier.apply(job.clone(), result.clone()).await,
                temper_engine::ApplyOutcome::Retryable { .. }
            ));

            let mut missing = job;
            missing.repo = "acme/missing".to_string();
            assert_eq!(
                applier.apply(missing, result).await,
                temper_engine::ApplyOutcome::Stale
            );
            assert!(rx.try_recv().is_none());
            let after_apply = issue_metadata(&forge, &repo, issue).await;
            assert_eq!(after_apply.assignment, claimed.assignment);
            assert_eq!(after_apply.lease, claimed.lease);
        })
    });
    let backend_error = diagnostic(
        &events,
        Level::ERROR,
        "apply",
        "lease applier repository lookup failed",
        ("error", "diagnostic apply repository lookup failure"),
    );
    assert!(backend_error.field("error").is_some());
    let absence = diagnostic(
        &events,
        Level::WARN,
        "apply",
        "lease applier assignment target no longer exists",
        ("repo", "acme/missing"),
    );
    assert_eq!(absence.field("repo"), Some("acme/missing"));
    assert!(absence.field("error").is_none());
}

#[test]
fn heartbeat_missing_repository_preserves_durable_metadata() {
    let events = capture_events(|| {
        temper_engine_io::block_on_with(move |_cx, _handle| async move {
            let forge = Arc::new(MemoryForge::new());
            let repo = new_repo(&forge).await;
            let issue = create_ready_issue(&forge, &repo).await;
            let (inner, mut rx) = recording_inner();
            let initial = "2026-07-15T14:50:00Z"
                .parse::<DateTime<Utc>>()
                .expect("valid initial time");
            let (now, clock) = controlled_clock(initial);
            let applier = LeaseApplier::new(forge.clone(), policy(), "daemon-1", inner, clock);
            let job = in_flight_job(issue);
            let context = claim_context();
            assert_eq!(
                applier.claim(job.clone(), context.clone()).await,
                temper_engine::ClaimOutcome::Claimed
            );
            let claimed = issue_metadata(&forge, &repo, issue).await;

            *now.lock().expect("clock lock") = initial + chrono::Duration::seconds(60);
            forge.fail_next(
                FaultOp::GetRepositoryByPath,
                "diagnostic heartbeat repository lookup failure",
            );
            assert!(matches!(
                applier.heartbeat(job.clone(), context.clone()).await,
                RecoveredHeartbeatOutcome::TransientlyUnavailable { reason }
                    if reason.contains("diagnostic heartbeat repository lookup failure")
            ));

            *now.lock().expect("clock lock") = initial + chrono::Duration::seconds(120);
            let mut missing = job;
            missing.repo = "acme/missing".to_string();
            assert_eq!(
                applier.heartbeat(missing, context).await,
                RecoveredHeartbeatOutcome::OwnershipLost {
                    reason: RecoveredOwnershipLossReason::TargetRemoved,
                }
            );

            let after_heartbeats = issue_metadata(&forge, &repo, issue).await;
            assert_eq!(after_heartbeats.assignment, claimed.assignment);
            assert_eq!(after_heartbeats.lease, claimed.lease);
            assert!(rx.try_recv().is_none());
        })
    });
    let backend_error = diagnostic(
        &events,
        Level::ERROR,
        "heartbeat",
        "lease applier repository lookup failed",
        ("error", "diagnostic heartbeat repository lookup failure"),
    );
    assert!(backend_error.field("error").is_some());
    let absence = diagnostic(
        &events,
        Level::WARN,
        "heartbeat",
        "recovered assignment heartbeat target no longer exists",
        ("repo", "acme/missing"),
    );
    assert_eq!(absence.field("repo"), Some("acme/missing"));
    assert!(absence.field("error").is_none());
}

#[test]
fn release_missing_repository_drops_local_context_and_preserves_durable_metadata() {
    let events = capture_events(|| {
        temper_engine_io::block_on_with(move |_cx, _handle| async move {
            let forge = Arc::new(MemoryForge::new());
            let repo = new_repo(&forge).await;
            let issue = create_ready_issue(&forge, &repo).await;
            let (inner, mut rx) = recording_inner();
            let applier = LeaseApplier::new(
                forge.clone(),
                policy(),
                "daemon-1",
                inner,
                temper_engine::system_clock(),
            );
            let job = in_flight_job(issue);
            let context = claim_context();
            assert_eq!(
                applier.claim(job.clone(), context.clone()).await,
                temper_engine::ClaimOutcome::Claimed
            );
            let claimed = issue_metadata(&forge, &repo, issue).await;

            let mut missing = job.clone();
            missing.repo = "acme/missing".to_string();
            applier.release_claim(missing, context.clone()).await;
            assert_eq!(
                applier.apply(job.clone(), job_result(&job.job_id)).await,
                temper_engine::ApplyOutcome::Stale
            );
            assert!(rx.try_recv().is_none());

            forge.fail_next(
                FaultOp::GetRepositoryByPath,
                "diagnostic release repository lookup failure",
            );
            applier.release_claim(job, context).await;

            let after_release = issue_metadata(&forge, &repo, issue).await;
            assert_eq!(after_release.assignment, claimed.assignment);
            assert_eq!(after_release.lease, claimed.lease);
        })
    });
    let backend_error = diagnostic(
        &events,
        Level::ERROR,
        "release",
        "lease applier repository lookup failed; durable assignment cleanup deferred to lease expiry and live reconciliation",
        ("error", "diagnostic release repository lookup failure"),
    );
    assert!(backend_error.field("error").is_some());
    let absence = diagnostic(
        &events,
        Level::WARN,
        "release",
        "lease applier assignment target no longer exists; durable assignment cleanup deferred to lease expiry and live reconciliation",
        ("repo", "acme/missing"),
    );
    assert_eq!(absence.field("repo"), Some("acme/missing"));
    assert!(absence.field("error").is_none());
}
