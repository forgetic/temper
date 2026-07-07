// SPDX-License-Identifier: MPL-2.0

use crate::assertions::*;
use crate::support::*;

use std::collections::BTreeMap;
use std::sync::{Arc as StdArc, Mutex};

use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry;

#[derive(Clone, Debug, Default)]
struct CapturedEvent {
    target: String,
    fields: BTreeMap<String, String>,
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

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

#[derive(Clone, Default)]
struct CaptureLayer {
    events: StdArc<Mutex<Vec<CapturedEvent>>>,
}

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = CapturedVisitor::default();
        event.record(&mut visitor);
        self.events.lock().unwrap().push(CapturedEvent {
            target: event.metadata().target().to_string(),
            fields: visitor.fields,
        });
    }
}

#[test]
fn triage_verdict_success_rewrites_body_and_routes_labels_without_pr() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let compiled = workflow.compile();
        let applier = Arc::new(LeaseApplier::new(
            forge.clone(),
            policy(),
            "daemon-1",
            Arc::new(ForgeApplier::new(forge.clone(), workflow.clone())),
            temper_engine::system_clock(),
        ));
        let daemon = Daemon::with_applier(Arc::new(handle.clone()), applier);
        let url = spawn(&handle, &daemon).await;
        let client = temper_engine_io::http::JsonClient::new();
        let role = RoleId::new("architect");

        assert_eq!(
            post(
                &client,
                &url,
                &register("worker-a", "architect", "acme/service")
            )
            .await
            .status,
            204
        );

        assert_eq!(
            daemon
                .enqueue_scanned_role_work(
                    forge.as_ref(),
                    &repo,
                    workflow.as_ref(),
                    &compiled,
                    ts("2026-05-29T00:00:00Z"),
                    &role,
                    RoleFeedMode::Normal,
                )
                .await
                .expect("feed succeeds"),
            1
        );
        let assignment =
            poll_assignment_for_role(&client, &url, "worker-a", "architect", "issue", issue).await;
        let context: JobContext = serde_json::from_value(assignment.job_payload.clone())
            .expect("assignment payload is a JobContext");
        assert_eq!(context.action.as_deref(), Some("triage_intake"));
        assert_eq!(
            context.allowed_verdicts,
            vec!["needs_breakdown", "needs_design", "ready_code"]
        );
        assert_eq!(context.checkout_capability.as_deref(), Some("read_only"));

        let result = verdict_result(
            "worker-a",
            &assignment.job_id,
            "ready_code",
            Some("rewritten spec"),
        );
        assert_release(
            post_json(&client, &url, &WorkerProtocolMessage::Result(result)).await,
            "worker-a",
            &assignment.job_id,
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        let (body, labels) = loop {
            let state = issue_body_and_labels(&forge, &repo, issue).await;
            if state.0 == "rewritten spec" {
                break state;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for verdict apply, saw body {:?} labels {:?}",
                state.0,
                state.1
            );
            temper_engine_io::runtime::sleep_for(&cx, Duration::from_millis(10)).await;
        };

        assert_eq!(body, "rewritten spec");
        assert!(!labels.iter().any(|label| label == "untriaged"));
        assert!(labels.iter().any(|label| label == "code"));
        assert!(labels.iter().any(|label| label == "ready"));
        assert_no_pull_requests(&forge, &repo).await;
    })
}

#[test]
fn triage_verdict_success_emits_transition_applied_for_routed_outcome() {
    let layer = CaptureLayer::default();
    let events = layer.events.clone();
    tracing::subscriber::set_global_default(registry().with(layer))
        .expect("install capture subscriber");

    let runtime = temper_engine_io::build_runtime().expect("build engine runtime");
    let source_ref = runtime.block_on(async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
        let source_ref =
            temper_runner::artifact_ref(&repo, ArtifactSource::Issue { number: issue }).to_string();
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = triage_in_flight_job("acme/service", issue);
        let result = verdict_result(
            "worker-a",
            &job.job_id,
            "ready_code",
            Some("rewritten spec"),
        );

        applier.apply(job, result).await;

        let (body, labels) = issue_body_and_labels(&forge, &repo, issue).await;
        assert_eq!(body, "rewritten spec");
        assert!(!labels.iter().any(|label| label == "untriaged"));
        assert!(labels.iter().any(|label| label == "code"));
        assert!(labels.iter().any(|label| label == "ready"));
        source_ref
    });

    let captured = events.lock().unwrap();
    let transition_position = captured
        .iter()
        .position(|event| {
            event.fields.get("event").map(String::as_str) == Some("transition.applied")
                && event.fields.get("transition").map(String::as_str)
                    == Some("triage_intake_to_code")
                && event.fields.get("artifact.ref").map(String::as_str) == Some(source_ref.as_str())
        })
        .unwrap_or_else(|| {
            panic!("missing routed verdict transition.applied event in {captured:#?}")
        });
    let transition = &captured[transition_position];
    assert_eq!(transition.target, "temper::engine");
    assert_eq!(
        transition.fields.get("service").map(String::as_str),
        Some("engine")
    );
    assert_eq!(
        transition.fields.get("labels.delta").map(String::as_str),
        Some("-untriaged +code +ready")
    );
}

#[test]
fn triage_verdict_replay_is_quiet_no_op() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge, "stable").await;
        let issue = create_untriaged_intake_issue(&forge, &repo).await;
        let workflow = Arc::new(workflow());
        let applier = ForgeApplier::new(forge.clone(), workflow);
        let job = triage_in_flight_job("acme/service", issue);
        let result = verdict_result(
            "worker-a",
            &job.job_id,
            "ready_code",
            Some("rewritten spec"),
        );

        applier.apply(job.clone(), result.clone()).await;
        let after_first = issue_body_and_labels(&forge, &repo, issue).await;
        applier.apply(job, result).await;
        let after_second = issue_body_and_labels(&forge, &repo, issue).await;

        assert_eq!(after_first, after_second);
        assert_eq!(after_second.0, "rewritten spec");
        assert!(!after_second.1.iter().any(|label| label == "untriaged"));
        assert!(after_second.1.iter().any(|label| label == "code"));
        assert!(after_second.1.iter().any(|label| label == "ready"));
        assert_no_pull_requests(&forge, &repo).await;
    })
}
